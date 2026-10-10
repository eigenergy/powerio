//! The committed current PowerIO IR schema matches the implementation, and
//! the historical schema archive remains intact.

use std::path::Path;

const SCHEMA_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/schema");
const CURRENT_SCHEMA: &str = "pio-ir/2/0.11.6/schema.json";

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
            "pio-ir/2/0.11.1/schema.json",
            "pio-ir/2/0.11.3/schema.json",
            "pio-ir/2/0.11.4/schema.json",
            "pio-ir/2/0.11.5/schema.json",
            CURRENT_SCHEMA,
            "pio-ir/2/schema.json",
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
        (
            "pio-ir/2/schema.json",
            "https://powerio.dev/schema/pio-ir/2/schema.json",
        ),
        (
            "pio-ir/2/0.11.1/schema.json",
            "https://powerio.dev/schema/pio-ir/2/0.11.1/schema.json",
        ),
        (
            "pio-ir/2/0.11.3/schema.json",
            "https://powerio.dev/schema/pio-ir/2/0.11.3/schema.json",
        ),
        (
            "pio-ir/2/0.11.4/schema.json",
            "https://powerio.dev/schema/pio-ir/2/0.11.4/schema.json",
        ),
        (
            "pio-ir/2/0.11.5/schema.json",
            "https://powerio.dev/schema/pio-ir/2/0.11.5/schema.json",
        ),
    ] {
        let schema: serde_json::Value = serde_json::from_str(&read_schema_file(path)).unwrap();
        assert_eq!(schema["$id"], expected_id, "historical schema {path}");
    }
}

/// The current schema uses the public PowerIO IR identity and version.
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

/// Each earlier IR version 2 schema file is checked against the current one.
/// A release can add structural types and definitions, and a list of allowed
/// string values (such as the distribution source formats) can gain values.
/// Every other existing definition, and the document rules, stay byte for
/// byte the same, so a document an earlier 0.11 release wrote still reads
/// with the same meaning. Renaming or reshaping an existing type fails here.
#[test]
fn ir_version_2_schemas_only_add() {
    for earlier in [
        "pio-ir/2/0.11.5/schema.json",
        "pio-ir/2/0.11.4/schema.json",
        "pio-ir/2/schema.json",
        "pio-ir/2/0.11.1/schema.json",
        "pio-ir/2/0.11.3/schema.json",
    ] {
        let mut published: serde_json::Value =
            serde_json::from_str(&read_schema_file(earlier)).unwrap();
        let mut current: serde_json::Value =
            serde_json::from_str(&read_schema_file(CURRENT_SCHEMA)).unwrap();
        let published_defs = published.as_object_mut().unwrap().remove("$defs").unwrap();
        let current_defs = current.as_object_mut().unwrap().remove("$defs").unwrap();
        published.as_object_mut().unwrap().remove("$id");
        current.as_object_mut().unwrap().remove("$id");
        assert_eq!(published, current, "{earlier}: document rules changed");

        for (name, definition) in published_defs.as_object().unwrap() {
            if name == "StoredValue" {
                for variant in definition["oneOf"].as_array().unwrap() {
                    let type_name = &variant["properties"]["type"]["const"];
                    let matching = current_defs[name]["oneOf"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|candidate| candidate["properties"]["type"]["const"] == *type_name);
                    assert_eq!(
                        matching,
                        Some(variant),
                        "{earlier}: type {type_name} changed"
                    );
                }
            } else if name == "DistLoadVoltageModel" {
                assert_additive_load_laws(earlier, definition, &current_defs[name]);
            } else if name == "MulticonductorNetwork" {
                // Component geography uses a new mandatory nested type tag.
                // The legacy network branch and the nested electrical body
                // keep every published field and meaning unchanged.
                let branches = current_defs[name]["anyOf"].as_array().unwrap();
                assert_eq!(branches.len(), 2);
                assert_eq!(
                    &branches[0], definition,
                    "{earlier}: legacy network changed"
                );
                assert_eq!(branches[1]["$ref"], "#/$defs/LocatedNetworkWire");
                assert_eq!(&current_defs["MulticonductorNetworkBody"], definition);
                assert_eq!(
                    current_defs["LocatedNetworkTag"]["enum"],
                    serde_json::json!(["powerio.LocatedMulticonductorNetwork"])
                );
                assert_eq!(
                    current_defs["LocatedNetworkWire"]["required"],
                    serde_json::json!(["type", "value"])
                );
            } else if name == "VoltageSource" {
                // IR 2 also admits a distinctly tagged nested source type.
                // Keep the legacy branch byte-for-byte equivalent as JSON;
                // do not generally permit arbitrary changes to record unions.
                let branches = current_defs[name]["anyOf"].as_array().unwrap();
                assert_eq!(branches.len(), 2);
                assert_eq!(&branches[0], definition, "{earlier}: legacy source changed");
                assert_eq!(branches[1]["$ref"], "#/$defs/ReferencedSourceWire");
                let new_source = &current_defs["ReferencedSourceWire"]["oneOf"][0];
                assert_eq!(
                    new_source["properties"]["type"]["const"],
                    "powerio.ReferencedVoltageSource"
                );
                assert_eq!(new_source["required"], serde_json::json!(["type", "value"]));
            } else if let Some(values) = definition["enum"].as_array() {
                // A list of allowed values keeps every value it had; it may
                // gain values, and nothing else about it changes.
                let current = &current_defs[name];
                let current_values = current["enum"].as_array().unwrap_or_else(|| {
                    panic!("{earlier}: {name} is no longer a list of allowed values")
                });
                for value in values {
                    assert!(
                        current_values.contains(value),
                        "{earlier}: {name} no longer allows {value}"
                    );
                }
                let without_values = |definition: &serde_json::Value| {
                    let mut definition = definition.clone();
                    definition.as_object_mut().unwrap().remove("enum");
                    definition
                };
                assert_eq!(
                    without_values(current),
                    without_values(definition),
                    "{earlier}: {name} changed beyond gaining values"
                );
            } else {
                assert_eq!(
                    &current_defs[name], definition,
                    "{earlier}: definition {name} changed"
                );
            }
        }
    }
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

fn assert_additive_load_laws(
    earlier: &str,
    published: &serde_json::Value,
    current: &serde_json::Value,
) {
    // Explicit discriminators add electrical laws without changing any
    // published variant. Pin each addition and its fields, rather than
    // accepting an arbitrary increase in the enum's variant count.
    let old = published["oneOf"].as_array().unwrap();
    let new = current["oneOf"].as_array().unwrap();
    let additions: [(&str, &[&str], &[&str]); 2] = [
        (
            "bounded_zip",
            &[
                "model",
                "v_nom",
                "alpha_z",
                "alpha_i",
                "alpha_p",
                "beta_z",
                "beta_i",
                "beta_p",
                "v_low_pu",
                "v_min_pu",
                "v_max_pu",
                "cutoff_pu",
            ],
            &[],
        ),
        (
            "bounded_exponential",
            &[
                "model", "v_nom", "gamma_p", "gamma_q", "v_low_pu", "v_min_pu", "v_max_pu",
            ],
            &[],
        ),
    ];
    assert_eq!(new.len(), old.len() + additions.len());
    for variant in old {
        let tag = &variant["properties"]["model"]["const"];
        assert_eq!(
            new.iter()
                .find(|candidate| candidate["properties"]["model"]["const"] == *tag),
            Some(variant),
            "{earlier}: load law {tag} changed",
        );
    }
    let nominal_voltage = &old
        .iter()
        .find(|variant| variant["properties"]["model"]["const"] == "constant_power")
        .unwrap()["properties"]["v_nom"];
    for (tag, required, optional) in additions {
        assert!(
            !old.iter()
                .any(|variant| variant["properties"]["model"]["const"] == tag),
            "{earlier}: {tag} was already published",
        );
        let added = new
            .iter()
            .find(|variant| variant["properties"]["model"]["const"] == tag)
            .unwrap();
        assert_eq!(added["type"], "object");
        assert_eq!(added["required"], serde_json::json!(required));
        let fields = added["properties"].as_object().unwrap();
        let mut expected_fields: Vec<_> = required.iter().chain(optional).copied().collect();
        expected_fields.sort_unstable();
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            expected_fields
        );
        for (name, schema) in fields {
            let expected = match name.as_str() {
                "model" => serde_json::json!({"type": "string", "const": tag}),
                "v_nom" | "alpha_z" | "alpha_i" | "alpha_p" | "beta_z" | "beta_i" | "beta_p"
                | "gamma_p" | "gamma_q" => nominal_voltage.clone(),
                "v_low_pu" | "v_min_pu" | "v_max_pu" | "cutoff_pu" => {
                    nominal_voltage["items"].clone()
                }
                _ => panic!("unexpected {tag} field {name}"),
            };
            assert_eq!(schema, &expected, "{tag}.{name} schema changed");
        }
    }
}

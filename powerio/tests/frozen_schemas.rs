//! The committed current PowerIO IR schema matches the implementation, and
//! the historical schema archive remains intact.

use std::path::Path;

const SCHEMA_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/schema");
const CURRENT_SCHEMA: &str = "pio-ir/2/0.11.8/schema.json";

fn monitor_discriminator(value: &serde_json::Value) -> Option<(&str, &str)> {
    if value["type"] != "object" {
        return None;
    }
    let required = value["required"].as_array()?;
    for name in ["kind", "type"] {
        if required.iter().any(|field| field == name)
            && let Some(tag) = value["properties"][name]["const"].as_str()
        {
            return Some((name, tag));
        }
    }
    None
}

fn assert_monitor_properties_preserved(
    published: &serde_json::Value,
    current: &serde_json::Value,
    required: Option<&Vec<serde_json::Value>>,
    path: &str,
) {
    let old = published.as_object().expect("published properties object");
    let new = current.as_object().expect("current properties object");
    for (property, old_schema) in old {
        let new_schema = new.get(property).unwrap_or_else(|| {
            panic!("{path}.properties.{property}: published property was removed")
        });
        assert_monitor_schema_preserved(
            old_schema,
            new_schema,
            &format!("{path}.properties.{property}"),
        );
    }
    for property in new.keys().filter(|property| !old.contains_key(*property)) {
        assert!(
            !required.is_some_and(|fields| fields.iter().any(|field| field == property)),
            "{path}.properties.{property}: additive property became required"
        );
    }
}

fn assert_monitor_alternatives_preserved(
    old: &[serde_json::Value],
    new: &[serde_json::Value],
    path: &str,
) {
    let Some((tag_property, _)) = old.first().and_then(monitor_discriminator) else {
        assert_eq!(old, new, "{path}: non-discriminated alternatives changed");
        return;
    };
    let mut alternatives = std::collections::BTreeMap::new();
    for item in new {
        let (property, tag) = monitor_discriminator(item).unwrap_or_else(|| {
            panic!("{path}: variant must require a constant kind/type tag on an object")
        });
        assert_eq!(
            property, tag_property,
            "{path}: discriminator property changed"
        );
        assert!(
            alternatives.insert(tag, item).is_none(),
            "{path}: duplicate discriminator {tag} overlaps oneOf alternatives"
        );
    }
    for item in old {
        let (property, tag) = monitor_discriminator(item).expect("tagged published variant");
        assert_eq!(
            property, tag_property,
            "{path}: mixed published discriminators"
        );
        let matching = alternatives
            .get(tag)
            .unwrap_or_else(|| panic!("{path}: published variant {tag} was removed"));
        assert_monitor_schema_preserved(item, matching, path);
    }
}

fn assert_monitor_schema_preserved(
    published: &serde_json::Value,
    current: &serde_json::Value,
    path: &str,
) {
    let keyword = path.rsplit('.').next();
    match (published, current) {
        (serde_json::Value::Object(old), serde_json::Value::Object(new)) => {
            for (name, value) in old {
                let current_value = new.get(name).unwrap_or_else(|| {
                    panic!("{path}.{name}: published schema member was removed")
                });
                if name == "properties" {
                    assert_monitor_properties_preserved(
                        value,
                        current_value,
                        current["required"].as_array(),
                        path,
                    );
                } else {
                    assert_monitor_schema_preserved(
                        value,
                        current_value,
                        &format!("{path}.{name}"),
                    );
                }
            }
            for name in new.keys().filter(|name| !old.contains_key(*name)) {
                assert!(
                    matches!(
                        name.as_str(),
                        "description"
                            | "title"
                            | "examples"
                            | "default"
                            | "deprecated"
                            | "readOnly"
                            | "writeOnly"
                            | "$comment"
                    ),
                    "{path}.{name}: unapproved schema keyword was added"
                );
            }
        }
        (serde_json::Value::Array(old), serde_json::Value::Array(new))
            if matches!(keyword, Some("oneOf" | "anyOf")) =>
        {
            assert_monitor_alternatives_preserved(old, new, path);
        }
        (serde_json::Value::Array(old), serde_json::Value::Array(new))
            if keyword == Some("enum") =>
        {
            for value in old {
                assert!(
                    new.contains(value),
                    "{path}: published enum value {value} was removed"
                );
            }
        }
        _ => assert_eq!(published, current, "{path}: published schema changed"),
    }
}

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
            "pio-ir/2/0.11.6/schema.json",
            "pio-ir/2/0.11.7/schema.json",
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
        (
            "pio-ir/2/0.11.6/schema.json",
            "https://powerio.dev/schema/pio-ir/2/0.11.6/schema.json",
        ),
        (
            "pio-ir/2/0.11.7/schema.json",
            "https://powerio.dev/schema/pio-ir/2/0.11.7/schema.json",
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

fn assert_partial_transformer_schema_preserved(
    definition: &serde_json::Value,
    current_defs: &serde_json::Value,
    earlier: &str,
) {
    // Nondefault winding states have their own tagged nested record;
    // the complete historical transformer body stays unchanged.
    let branches = current_defs["Transformer3W"]["anyOf"].as_array().unwrap();
    assert_eq!(branches.len(), 2);
    assert_eq!(
        &branches[0], definition,
        "{earlier}: legacy transformer changed"
    );
    assert_eq!(branches[1]["$ref"], "#/$defs/PartialTransformer3WWire");
    let partial = &current_defs["PartialTransformer3WWire"]["oneOf"][0];
    assert_eq!(
        partial["properties"]["type"]["const"],
        "powerio.PartialTransformer3W"
    );
    assert_eq!(partial["required"], serde_json::json!(["type", "value"]));
    assert_eq!(
        partial["properties"]["value"]["$ref"],
        "#/$defs/PartialTransformer3WFields"
    );
    let mut partial_body = current_defs["PartialTransformer3WFields"].clone();
    let extras = partial_body["properties"]["extras"]
        .as_object_mut()
        .unwrap();
    assert_eq!(
        extras.remove("required"),
        Some(serde_json::json!(["winding_in_service"]))
    );
    assert_eq!(
        extras.remove("properties"),
        Some(serde_json::json!({
            "winding_in_service": {"type": "array", "minItems": 3, "maxItems": 3,
                "items": {"type": "boolean"}, "not": {"const": [true, true, true]}}
        }))
    );
    assert_eq!(&partial_body, definition);
}

/// The power flow instance records gain one optional field in 0.11.7,
/// `hvdc_treatment`. Without that field the record is byte for byte the
/// earlier one, and the field is not required, so a document an earlier 0.11
/// release wrote still reads; the reader gives a document without the field
/// the treatment it denoted then.
fn assert_only_optional_property_added(
    published: &serde_json::Value,
    current: &serde_json::Value,
    property: &str,
    context: &str,
) {
    let mut without = current.clone();
    let removed = without["properties"]
        .as_object_mut()
        .unwrap_or_else(|| panic!("{context}: no longer a record"))
        .remove(property);
    assert!(removed.is_some(), "{context}: {property} is missing");
    assert!(
        !current["required"]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|field| field == property)),
        "{context}: {property} became required"
    );
    assert_eq!(
        &without, published,
        "{context} changed beyond gaining {property}"
    );
}

/// The constants that tag one alternative of a union: string constants
/// (such as source format names, which may share one alternative as a list
/// of allowed values), or an object's one required property with a constant
/// value (such as an objective term's `term`), named with that property.
fn alternative_tags(alternative: &serde_json::Value) -> Option<(Option<&str>, Vec<&str>)> {
    if let Some(tag) = alternative["const"].as_str() {
        return Some((None, vec![tag]));
    }
    if let Some(values) = alternative["enum"].as_array() {
        return values
            .iter()
            .map(serde_json::Value::as_str)
            .collect::<Option<Vec<_>>>()
            .map(|tags| (None, tags));
    }
    let mut tags = alternative["required"]
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|property| {
            alternative["properties"][property]["const"]
                .as_str()
                .map(|tag| (Some(property), vec![tag]))
        });
    let tag = tags.next()?;
    tags.next().is_none().then_some(tag)
}

/// A union whose alternatives each carry a constant tag may gain
/// alternatives, as a list of allowed values may gain values: every
/// alternative the earlier schema file states stays byte for byte the same,
/// every alternative carries a tag on the same property, no two share a tag,
/// and nothing else about the definition changes. A document an earlier
/// release wrote still matches exactly one alternative with the same
/// meaning.
fn assert_tagged_union_only_adds(
    published: &serde_json::Value,
    current: &serde_json::Value,
    context: &str,
) {
    if published == current {
        return;
    }
    let old = published["oneOf"].as_array().unwrap();
    let new = current["oneOf"]
        .as_array()
        .unwrap_or_else(|| panic!("{context} is no longer a union of alternatives"));
    for alternative in old {
        assert!(
            new.contains(alternative),
            "{context}: an alternative of the earlier schema changed or was removed: {alternative}"
        );
    }
    let mut property = None;
    let mut tags = std::collections::BTreeSet::new();
    for alternative in new {
        let (tag_property, alternative_tags) = alternative_tags(alternative)
            .unwrap_or_else(|| panic!("{context}: an alternative carries no constant tag"));
        assert_eq!(
            *property.get_or_insert(tag_property),
            tag_property,
            "{context}: alternatives are tagged on different properties"
        );
        for tag in alternative_tags {
            assert!(
                tags.insert(tag),
                "{context}: two alternatives share the tag {tag}"
            );
        }
    }
    let without_alternatives = |definition: &serde_json::Value| {
        let mut definition = definition.clone();
        definition.as_object_mut().unwrap().remove("oneOf");
        definition
    };
    assert_eq!(
        without_alternatives(current),
        without_alternatives(published),
        "{context} changed beyond gaining alternatives"
    );
}

/// Each earlier IR version 2 schema file is checked against the current one.
/// A release can add structural types and definitions, a list of allowed
/// string values (such as the distribution source formats) can gain values,
/// and a union of constant-tagged alternatives (such as the source formats
/// and the objective terms) can gain alternatives.
/// Every other existing definition, and the document rules, stay byte for
/// byte the same, so a document an earlier 0.11 release wrote still reads
/// with the same meaning. Renaming or reshaping an existing type fails here.
#[test]
#[allow(clippy::too_many_lines)]
fn ir_version_2_schemas_only_add() {
    for earlier in [
        "pio-ir/2/0.11.7/schema.json",
        "pio-ir/2/0.11.6/schema.json",
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
            // An unchanged definition satisfies every rule below; the rules
            // describe how a definition may differ from an earlier file.
            if current_defs.get(name) == Some(definition) {
                continue;
            }
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
                assert_located_network_schema_preserved(earlier, definition, &current_defs);
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
            } else if name == "Transformer3W" {
                assert_partial_transformer_schema_preserved(definition, &current_defs, earlier);
            } else if name == "DcPfInstance" || name == "AcPfInstance" {
                assert_only_optional_property_added(
                    definition,
                    &current_defs[name],
                    "hvdc_treatment",
                    &format!("{earlier}: record {name}"),
                );
            } else if name == "MonitorScope" || name == "MonitorStatement" {
                assert_monitor_schema_preserved(
                    definition,
                    &current_defs[name],
                    &format!("{earlier}: record {name}"),
                );
            } else if definition["oneOf"].is_array() {
                assert_tagged_union_only_adds(
                    definition,
                    &current_defs[name],
                    &format!("{earlier}: {name}"),
                );
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

#[test]
fn monitor_schema_additions_preserve_published_variants() {
    let published: serde_json::Value =
        serde_json::from_str(&read_schema_file("pio-ir/2/0.11.5/schema.json")).unwrap();
    let current: serde_json::Value =
        serde_json::from_str(&read_schema_file(CURRENT_SCHEMA)).unwrap();
    for name in ["MonitorScope", "MonitorStatement"] {
        assert_monitor_schema_preserved(&published["$defs"][name], &current["$defs"][name], name);
    }
}

#[test]
fn monitor_schema_guard_rejects_new_restrictions_and_allows_additive_shapes() {
    let published = serde_json::json!({"type": "string"});
    for (keyword, value) in [
        ("maxLength", serde_json::json!(0)),
        ("pattern", serde_json::json!("^")),
        ("maximum", serde_json::json!(0)),
        ("maxItems", serde_json::json!(0)),
    ] {
        let mut narrowed = published.clone();
        narrowed
            .as_object_mut()
            .unwrap()
            .insert(keyword.to_owned(), value);
        assert!(
            std::panic::catch_unwind(|| {
                assert_monitor_schema_preserved(&published, &narrowed, "probe");
            })
            .is_err(),
            "new validation restriction {keyword} must not be silently accepted"
        );
    }

    let published_object = serde_json::json!({
        "type": "object",
        "properties": {"kind": {"const": "old", "type": "string"}},
        "required": ["kind"]
    });
    let additive_object = serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"const": "old", "type": "string"},
            "label": {"type": "string"}
        },
        "required": ["kind"]
    });
    assert_monitor_schema_preserved(&published_object, &additive_object, "probe");

    let restricted_object = serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"const": "old", "type": "string"},
            "label": {"type": "string"}
        },
        "required": ["kind", "label"]
    });
    assert!(
        std::panic::catch_unwind(|| {
            assert_monitor_schema_preserved(&published_object, &restricted_object, "probe");
        })
        .is_err(),
        "a newly added property must not become required"
    );

    let old_variants = serde_json::json!({"oneOf": [
        {"properties": {"kind": {"const": "old", "type": "string"}}, "required": ["kind"], "type": "object"}
    ]});
    let extended_variants = serde_json::json!({"oneOf": [
        {"properties": {"kind": {"const": "old", "type": "string"}}, "required": ["kind"], "type": "object"},
        {"properties": {"kind": {"const": "new", "type": "string"}}, "required": ["kind"], "type": "object"}
    ]});
    assert_monitor_schema_preserved(&old_variants, &extended_variants, "probe");
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
    let additions: [(&str, &[&str], &[&str]); 3] = [
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
        (
            "bounded_constant_power",
            &["model", "v_nom", "v_min_pu", "v_max_pu"],
            &["current_limit_pu"],
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
                "current_limit_pu" => serde_json::json!({
                    "anyOf": [
                        {"format": "double", "type": ["number", "null"]},
                        {"enum": ["Infinity", "-Infinity", "NaN"], "type": "string"},
                    ],
                }),
                "v_low_pu" | "v_min_pu" | "v_max_pu" | "cutoff_pu" => {
                    nominal_voltage["items"].clone()
                }
                _ => panic!("unexpected {tag} field {name}"),
            };
            assert_eq!(schema, &expected, "{tag}.{name} schema changed");
        }
    }
}

fn tagged_monitor_variant(property: &str, tag: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {property: {"const": tag, "type": "string"}},
        "required": [property]
    })
}

#[test]
fn monitor_guard_rejects_overlapping_tagged_variants() {
    let variant = tagged_monitor_variant("kind", "old");
    let old = serde_json::json!({"oneOf": [variant.clone()]});
    let mut optional_tag = tagged_monitor_variant("kind", "new");
    optional_tag["required"] = serde_json::json!([]);
    let mut non_object = tagged_monitor_variant("kind", "new");
    non_object.as_object_mut().unwrap().remove("type");
    // Duplicates match the old object twice, and different discriminator
    // properties can both be present. Require one shared mandatory tag and
    // object shape so disjointness stays explicit for every added variant.
    for addition in [
        variant.clone(),
        tagged_monitor_variant("type", "new"),
        optional_tag,
        non_object,
    ] {
        let overlapping = serde_json::json!({"oneOf": [variant.clone(), addition]});
        assert!(
            std::panic::catch_unwind(|| {
                assert_monitor_schema_preserved(&old, &overlapping, "probe");
            })
            .is_err(),
            "potentially overlapping alternatives must not pass the compatibility guard"
        );
    }
}

#[test]
fn monitor_guard_allows_disjoint_tags_and_reordering() {
    for property in ["kind", "type"] {
        let first = tagged_monitor_variant(property, "first");
        let second = tagged_monitor_variant(property, "second");
        let new = tagged_monitor_variant(property, "new");
        let old = serde_json::json!({"oneOf": [first.clone(), second.clone()]});
        let extended = serde_json::json!({"oneOf": [new, second, first]});
        assert_monitor_schema_preserved(&old, &extended, "probe");
    }
}

fn assert_located_network_schema_preserved(
    earlier: &str,
    definition: &serde_json::Value,
    current_defs: &serde_json::Value,
) {
    // Component geography uses a new mandatory nested type tag.
    // The legacy network branch and the nested electrical body
    // keep every published field and meaning unchanged.
    let branches = current_defs["MulticonductorNetwork"]["anyOf"]
        .as_array()
        .unwrap();
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
}

fn objective_term_alternative(term: &str) -> serde_json::Value {
    serde_json::json!({
        "additionalProperties": false,
        "properties": {"term": {"const": term, "type": "string"}},
        "required": ["term"],
        "type": "object"
    })
}

#[test]
fn source_format_union_only_gains_disjoint_spellings() {
    let published = serde_json::json!({"oneOf": [
        {"type": "string", "enum": ["matpower", "psse"]},
        {"type": "string", "const": "sincal", "description": "existing"}
    ]});
    let mut grown = published.clone();
    grown["oneOf"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "type": "string", "const": "ravens-json", "description": "new"
        }));
    assert_tagged_union_only_adds(&published, &grown, "SourceFormat");
    for addition in [
        serde_json::json!({"type": "string", "const": "psse"}),
        serde_json::json!({"type": "string", "enum": ["sincal", "ravens-json"]}),
        serde_json::json!({"type": "string"}),
    ] {
        let mut invalid = published.clone();
        invalid["oneOf"].as_array_mut().unwrap().push(addition);
        assert!(
            std::panic::catch_unwind(|| {
                assert_tagged_union_only_adds(&published, &invalid, "SourceFormat");
            })
            .is_err()
        );
    }
    grown["oneOf"][1]["description"] = serde_json::json!("changed");
    assert!(
        std::panic::catch_unwind(|| {
            assert_tagged_union_only_adds(&published, &grown, "SourceFormat");
        })
        .is_err()
    );
}

#[test]
fn a_tagged_union_may_gain_alternatives_and_nothing_else() {
    let published = serde_json::json!({
        "description": "The PowerIO objective vocabulary.",
        "oneOf": [objective_term_alternative("network_generator_cost")]
    });
    let mut grown = published.clone();
    grown["oneOf"]
        .as_array_mut()
        .unwrap()
        .push(objective_term_alternative("branch_switching_cost"));
    assert_tagged_union_only_adds(&published, &grown, "probe");

    let mut changed = grown.clone();
    changed["oneOf"][0]["additionalProperties"] = serde_json::Value::Bool(true);
    let mut duplicate = published.clone();
    duplicate["oneOf"]
        .as_array_mut()
        .unwrap()
        .push(objective_term_alternative("network_generator_cost"));
    let mut untagged = published.clone();
    untagged["oneOf"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"type": "object"}));
    let mut other_property = published.clone();
    other_property["oneOf"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "properties": {"kind": {"const": "new", "type": "string"}},
            "required": ["kind"],
            "type": "object"
        }));
    let mut redescribed = grown.clone();
    redescribed["description"] = serde_json::Value::from("changed");
    for refused in [changed, duplicate, untagged, other_property, redescribed] {
        assert!(
            std::panic::catch_unwind(|| {
                assert_tagged_union_only_adds(&published, &refused, "probe");
            })
            .is_err(),
            "{refused} must not pass as an addition"
        );
    }
}

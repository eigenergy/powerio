//! The retired schema documents under `docs/schema/` are frozen. Old
//! `.pio.json` files declare their URLs, and the docs promise each URL
//! stays served. The `rust.yml` schemas job regenerates only the current
//! document, so it cannot catch a deletion here. This test pins the frozen
//! documents byte for byte. To retire one, change the pins and
//! `docs/schema/README.md` together.

/// FNV-1a 64. Implemented inline so the pin does not depend on a hasher whose
/// output could change across Rust or dependency versions.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The document for this build's stored module version has to be committed,
/// not merely generatable. The CI gate regenerates it and diffs
/// `docs/schema`, and a version bump writes a NEW directory, which a diff of
/// tracked files does not see. This fails under plain `cargo test` the moment
/// the version moves without the document following it.
#[test]
fn the_current_module_document_is_committed() {
    let version = powerio::stored::SCHEMA_VERSION;
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

#[test]
fn retired_schema_documents_stay_published_byte_for_byte() {
    let frozen: [(&str, usize, u64); 5] = [
        (
            "../docs/schema/pio-package/0.9/schema.json",
            187_720,
            0x0635_616a_e88c_cfbe,
        ),
        (
            "../docs/schema/pio-package/0.1/schema.json",
            125_750,
            0xe5af_9f64_b26e_edc2,
        ),
        (
            "../docs/schema/pio-package/0.2/schema.json",
            130_344,
            0x944c_3d7a_721d_1da9,
        ),
        (
            "../docs/schema/pio-payload-balanced/1/schema.json",
            51_415,
            0xe790_d6e1_ba75_a74f,
        ),
        (
            "../docs/schema/pio-payload-multiconductor/1/schema.json",
            35_414,
            0xabfe_0107_29fa_2afa,
        ),
    ];
    for (path, len, hash) in frozen {
        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            panic!(
                "{path} is a frozen schema document that pre-v0.8.0 .pio.json files \
                 reference by URL; it must stay published (see docs/schema/README.md). \
                 Could not read it: {e}"
            )
        });
        assert_eq!(
            (bytes.len(), fnv1a(&bytes)),
            (len, hash),
            "{path} changed, but it is frozen at the bytes its release published: documents in the wild \
             validate against it by URL, so edits belong in a NEW identifier path, not here \
             (see docs/schema/README.md)"
        );
    }
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
                if let Some(serde_json::Value::Object(props)) = map.get("properties") {
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

    let text = std::fs::read_to_string("../docs/schema/pio-module/1/schema.json").unwrap();
    let schema: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut bad = Vec::new();
    walk(&schema, "$", &mut bad);
    assert!(bad.is_empty(), "untyped properties: {bad:?}");
}

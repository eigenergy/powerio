//! Local OpenDSS corpus check.
//!
//! Skipped unless `POWERIO_DIST_LOCAL_DSS_CORPUS` points at a local tree
//! containing `.dss` files.

use std::path::{Path, PathBuf};

use powerio_dist::{dss::parse_dss_file, parse_bmopf_str, write_bmopf_json};

fn schema_validator() -> jsonschema::Validator {
    let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist/bmopf/draft_bmopf_schema.json");
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(schema_path).unwrap()).unwrap();
    jsonschema::validator_for(&schema).expect("vendored schema compiles")
}

fn collect_dss_files(root: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_dss_files(&path, out);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dss"))
        {
            out.push(path);
        }
    }
}

#[test]
fn local_dss_corpus_converts_to_valid_bmopf() {
    let Ok(root) = std::env::var("POWERIO_DIST_LOCAL_DSS_CORPUS") else {
        eprintln!("skipped: POWERIO_DIST_LOCAL_DSS_CORPUS is not set");
        return;
    };
    let root = PathBuf::from(root);
    let validator = schema_validator();
    let mut files = Vec::new();
    collect_dss_files(&root, &mut files);
    assert!(!files.is_empty(), "no .dss files under {}", root.display());

    let mut failures = Vec::new();
    let mut parse_warnings = 0usize;
    let mut write_warnings = 0usize;
    for path in &files {
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let net = match parse_dss_file(path) {
            Ok(net) => net,
            Err(err) => {
                failures.push(format!("{}: parse failed: {err}", rel.display()));
                continue;
            }
        };
        parse_warnings += net.warnings.len();
        let out = write_bmopf_json(&net);
        write_warnings += out.warnings.len();
        let doc = match serde_json::from_str::<serde_json::Value>(&out.text) {
            Ok(doc) => doc,
            Err(err) => {
                failures.push(format!("{}: BMOPF JSON parse failed: {err}", rel.display()));
                continue;
            }
        };
        let errors: Vec<_> = validator
            .iter_errors(&doc)
            .take(10)
            .map(|err| format!("{}: {err}", err.instance_path()))
            .collect();
        if !errors.is_empty() {
            failures.push(format!(
                "{}: BMOPF schema validation failed: {}",
                rel.display(),
                errors.join("; ")
            ));
            continue;
        }
        if let Err(err) = parse_bmopf_str(&out.text) {
            failures.push(format!("{}: BMOPF reparse failed: {err}", rel.display()));
        }
    }

    eprintln!(
        "checked {} .dss files; parse warnings: {parse_warnings}; BMOPF warnings: {write_warnings}",
        files.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

//! Local OpenDSS corpus check.
//!
//! Skipped unless `POWERIO_DIST_LOCAL_DSS_CORPUS` points at a local tree
//! containing `.dss` files.

use std::path::{Path, PathBuf};

use powerio_dist::{
    dss::{parse_dss_file, parse_dss_str},
    parse_bmopf_str, write_bmopf_json, write_dss,
};

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

fn validate_bmopf(
    validator: &jsonschema::Validator,
    rel: &Path,
    label: &str,
    text: &str,
    failures: &mut Vec<String>,
) -> Option<serde_json::Value> {
    let doc = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(doc) => doc,
        Err(err) => {
            failures.push(format!(
                "{}: {label} BMOPF JSON parse failed: {err}",
                rel.display()
            ));
            return None;
        }
    };
    let errors: Vec<_> = validator
        .iter_errors(&doc)
        .take(10)
        .map(|err| format!("{}: {err}", err.instance_path()))
        .collect();
    if !errors.is_empty() {
        failures.push(format!(
            "{}: {label} BMOPF schema validation failed: {}",
            rel.display(),
            errors.join("; ")
        ));
        return None;
    }
    Some(doc)
}

fn real_network_loss(warnings: &[String]) -> usize {
    warnings
        .iter()
        .filter(|w| {
            w.contains("not representable in the four BMOPF transformer subtypes; dropped")
                || (w.contains("reactor ")
                    && w.contains("class is not represented in BMOPF; dropped"))
                || (w.contains("capacitor ")
                    && w.contains("class is not represented in BMOPF; dropped"))
        })
        .count()
}

fn json_approx_eq(
    left: &serde_json::Value,
    right: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    match (left, right) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            if a.len() != b.len() {
                let only_a: Vec<_> = a.keys().filter(|k| !b.contains_key(*k)).cloned().collect();
                let only_b: Vec<_> = b.keys().filter(|k| !a.contains_key(*k)).cloned().collect();
                return Err(format!(
                    "{path}: object keys differ; only B1={only_a:?}; only B2={only_b:?}"
                ));
            }
            for (key, va) in a {
                let Some(vb) = b.get(key) else {
                    return Err(format!("{path}: key `{key}` is missing from B2"));
                };
                json_approx_eq(va, vb, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            if a.len() != b.len() {
                return Err(format!("{path}: array length {} != {}", a.len(), b.len()));
            }
            for (i, (va, vb)) in a.iter().zip(b).enumerate() {
                json_approx_eq(va, vb, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            let (Some(a), Some(b)) = (a.as_f64(), b.as_f64()) else {
                if left == right {
                    return Ok(());
                }
                return Err(format!("{path}: numeric value {left} != {right}"));
            };
            if (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0) {
                Ok(())
            } else {
                Err(format!("{path}: numeric value {a} != {b}"))
            }
        }
        _ if left == right => Ok(()),
        _ => Err(format!("{path}: {left:?} != {right:?}")),
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
    let mut real_losses = 0usize;
    for path in &files {
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let x = match parse_dss_file(path) {
            Ok(net) => net,
            Err(err) => {
                failures.push(format!("{}: parse failed: {err}", rel.display()));
                continue;
            }
        };
        parse_warnings += x.warnings.len();
        let b1 = write_bmopf_json(&x);
        write_warnings += b1.warnings.len();
        real_losses += real_network_loss(&b1.warnings);
        let Some(b1_doc) = validate_bmopf(&validator, rel, "B1", &b1.text, &mut failures) else {
            continue;
        };
        let y = match parse_bmopf_str(&b1.text) {
            Ok(net) => net,
            Err(err) => {
                failures.push(format!("{}: B1 BMOPF reparse failed: {err}", rel.display()));
                continue;
            }
        };
        let a2 = write_dss(&y);
        let z = parse_dss_str(&a2.text);
        let b2 = write_bmopf_json(&z);
        let Some(b2_doc) = validate_bmopf(&validator, rel, "B2", &b2.text, &mut failures) else {
            continue;
        };
        if let Err(diff) = json_approx_eq(&b1_doc, &b2_doc, "$") {
            failures.push(format!(
                "{}: BMOPF is not stable across D->B->D->B: {diff}",
                rel.display(),
            ));
        }
    }

    eprintln!(
        "checked {} .dss files; parse warnings: {parse_warnings}; BMOPF warnings: \
         {write_warnings}; real network losses: {real_losses}",
        files.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

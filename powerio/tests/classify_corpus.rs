//! Every JSON fixture in the corpus, classified, with its expected answer
//! stated here.
//!
//! A file picker in a consumer dispatches on this answer, so the classifier is
//! held to the whole corpus rather than to hand written snippets. Adding a
//! `.json` fixture without an entry fails the walk: a fixture whose class
//! nobody stated is a fixture nobody checked. An entry reading `ambiguous` or
//! `unknown` is a claim about the document, not a shrug — say beside it why
//! the markers cannot decide.

use std::path::{Path, PathBuf};

use powerio::format::routing::{JSON_CLASSES, JsonClass, classify_json_text};

/// Every `.json` file under `tests/data`, by path relative to that directory,
/// with the family the classifier must answer and the detected format where
/// there is one.
const EXPECTED: &[(&str, &str, Option<&str>)] = &[
    // Golden Arrow and matrix table dumps: neither is a case document, and
    // neither carries a case marker at top level.
    ("capi_arrow/case9_gen_cost.json", "unknown", None),
    ("capi_arrow/t_case9_dcline_gen_cost.json", "unknown", None),
    ("capi_matrix/case30_arrow_coo.json", "unknown", None),
    ("capi_matrix/case9_arrow_coo.json", "unknown", None),
    // The BMOPF schema document describes cases; it is not one.
    ("dist/bmopf/draft_bmopf_schema.json", "unknown", None),
    (
        "dist/bmopf/example_enwl_n1_f2.json",
        "distribution",
        Some("bmopf-json"),
    ),
    (
        "dist/bmopf/example_ieee13.json",
        "distribution",
        Some("bmopf-json"),
    ),
    (
        "dist/pmd/fourwire_linecode.json",
        "distribution",
        Some("pmd-json"),
    ),
    ("dist/pmd/ieee13.json", "distribution", Some("pmd-json")),
    ("egret/case14.json", "transmission", Some("egret-json")),
    ("egret/case30.json", "transmission", Some("egret-json")),
    ("egret/case9.json", "transmission", Some("egret-json")),
    ("egret/dcline3.json", "transmission", Some("egret-json")),
    (
        "opfdataset/example_0.json",
        "transmission",
        Some("opfdata-json"),
    ),
    (
        "pandapower/example.json",
        "transmission",
        Some("pandapower-json"),
    ),
    ("model-json/case30_v4.json", "model-json", None),
    // PyPSA sidecars: a coordinate reference system string and an empty
    // metadata object, both beside the CSV folder that carries the case.
    ("pypsa/example/crs.json", "unknown", None),
    ("pypsa/example/meta.json", "unknown", None),
];

fn data_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data")
}

fn json_fixtures(root: &Path, out: &mut Vec<String>, prefix: &str) {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .expect("the corpus directory must be readable")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        // `large/` is gitignored and fetched on demand, so it is not corpus.
        if name.starts_with('.') || name == "large" {
            continue;
        }
        let rel = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if path.is_dir() {
            json_fixtures(&path, out, &rel);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            out.push(rel);
        }
    }
}

#[test]
fn every_json_fixture_classifies_as_stated() {
    let root = data_root();
    let mut found = Vec::new();
    json_fixtures(&root, &mut found, "");

    for rel in &found {
        let (_, family, format) = EXPECTED
            .iter()
            .find(|(name, _, _)| name == rel)
            .unwrap_or_else(|| {
                panic!(
                    "{rel} has no expected class; add it to EXPECTED in \
                     powerio/tests/classify_corpus.rs"
                )
            });
        let text = std::fs::read_to_string(root.join(rel)).expect("fixture is readable");
        let class = classify_json_text(&text);
        assert_eq!(class.family(), *family, "{rel} classified as {class:?}");
        let detected = match class {
            JsonClass::Case(powerio::format::routing::Detection::Known(f)) => Some(f.name()),
            _ => None,
        };
        assert_eq!(detected, *format, "{rel} detected the wrong format");
        assert!(JSON_CLASSES.contains(&class.family()));
    }

    for (rel, _, _) in EXPECTED {
        assert!(
            found.iter().any(|f| f == rel),
            "{rel} is listed in EXPECTED but no longer exists in the corpus"
        );
    }
}

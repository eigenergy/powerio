//! The settled 1.0 crate graph, asserted from `cargo metadata` so a manifest
//! edit that reverses an edge or couples the sibling model crates fails here
//! rather than in review.
//!
//! The graph under test:
//!
//! ```text
//! powerio-core
//! ├── powerio-tx
//! └── powerio-dist
//!
//! powerio-prob   -> powerio-core + powerio-tx + powerio-dist
//! powerio-matrix -> powerio-core + powerio-tx + powerio-dist + powerio-prob
//! powerio        -> powerio-core + powerio-tx + powerio-dist + powerio-prob
//! powerio        -> powerio-matrix when the matrix feature is enabled
//! ```

use std::collections::{BTreeMap, BTreeSet};

/// Normal-dependency names per workspace crate, from `cargo metadata`.
/// Dev-dependencies are excluded: a dev edge (a test or bench convenience)
/// does not couple the published crates.
fn normal_dependencies() -> BTreeMap<String, BTreeSet<String>> {
    let cargo = std::env::var_os("CARGO").expect("cargo sets CARGO for test runs");
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata emits JSON");
    let mut graph = BTreeMap::new();
    for package in metadata["packages"].as_array().expect("packages array") {
        let name = package["name"].as_str().expect("package name").to_owned();
        let deps = package["dependencies"]
            .as_array()
            .expect("dependencies array")
            .iter()
            .filter(|dep| dep["kind"].is_null()) // normal deps only
            .map(|dep| dep["name"].as_str().expect("dependency name").to_owned())
            .collect();
        graph.insert(name, deps);
    }
    graph
}

fn workspace_deps_of<'g>(
    graph: &'g BTreeMap<String, BTreeSet<String>>,
    name: &str,
) -> BTreeSet<&'g str> {
    graph[name]
        .iter()
        .filter(|dep| graph.contains_key(*dep))
        .map(String::as_str)
        .collect()
}

#[test]
fn the_dependency_graph_matches_the_settled_layout() {
    let graph = normal_dependencies();

    // The foundation depends on no workspace crate.
    assert_eq!(workspace_deps_of(&graph, "powerio-core"), BTreeSet::new());

    // The two model crates are independent siblings over the foundation.
    assert_eq!(
        workspace_deps_of(&graph, "powerio-tx"),
        BTreeSet::from(["powerio-core"])
    );
    assert_eq!(
        workspace_deps_of(&graph, "powerio-dist"),
        BTreeSet::from(["powerio-core"])
    );

    // Problem data reads both models and stays matrix free.
    assert_eq!(
        workspace_deps_of(&graph, "powerio-prob"),
        BTreeSet::from(["powerio-core", "powerio-tx", "powerio-dist"])
    );

    // Matrix construction sits above the models and problem data, never on
    // or through the facade.
    assert_eq!(
        workspace_deps_of(&graph, "powerio-matrix"),
        BTreeSet::from(["powerio-core", "powerio-tx", "powerio-dist", "powerio-prob"])
    );

    // The facade pulls the component crates; matrix functionality is its
    // optional feature and must stay optional so `cargo add powerio` does not
    // drag the sparse dependencies unconditionally.
    let facade = &graph["powerio"];
    for required in ["powerio-core", "powerio-tx", "powerio-dist"] {
        assert!(facade.contains(required), "facade misses {required}");
    }
    assert!(
        facade.contains("powerio-matrix"),
        "facade lost its optional matrix dependency"
    );
}

#[test]
fn the_facade_matrix_dependency_is_optional() {
    let cargo = std::env::var_os("CARGO").expect("cargo sets CARGO for test runs");
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata runs");
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata emits JSON");
    let facade = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|package| package["name"] == "powerio")
        .expect("facade package");
    let matrix_dep = facade["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .find(|dep| dep["name"] == "powerio-matrix" && dep["kind"].is_null())
        .expect("facade names powerio-matrix");
    assert_eq!(matrix_dep["optional"], true, "matrix must be feature gated");
}

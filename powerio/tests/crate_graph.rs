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
//! powerio-sincal -> powerio-core
//! powerio-tx     -> powerio-sincal when the sincal feature is enabled
//! powerio-dist   -> powerio-sincal when the sincal feature is enabled
//! powerio-prob   -> powerio-core + powerio-tx + powerio-dist
//! powerio-matrix -> powerio-core + powerio-tx + powerio-dist + powerio-prob
//! powerio        -> powerio-core + powerio-tx + powerio-dist + powerio-prob
//! powerio        -> powerio-matrix when the matrix feature is enabled
//! powerio        -> powerio-sincal when the sincal feature is enabled
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

    // Native transport/schema support is model neutral. Family adapters can
    // depend on it without depending on one another.
    assert_eq!(
        workspace_deps_of(&graph, "powerio-sincal"),
        BTreeSet::from(["powerio-core"])
    );

    // The two model crates are independent siblings over the foundation.
    assert_eq!(
        workspace_deps_of(&graph, "powerio-tx"),
        BTreeSet::from(["powerio-core", "powerio-sincal"])
    );
    assert_eq!(
        workspace_deps_of(&graph, "powerio-dist"),
        BTreeSet::from(["powerio-core", "powerio-sincal"])
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

/// The normal dependency `dependency` of workspace `package`, from `cargo
/// metadata`, so a test can read its `optional` flag.
fn normal_dependency(package: &str, dependency: &str) -> serde_json::Value {
    let cargo = std::env::var_os("CARGO").expect("cargo sets CARGO for test runs");
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata runs");
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata emits JSON");
    metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|entry| entry["name"] == package)
        .unwrap_or_else(|| panic!("{package} is a workspace package"))["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .find(|dep| dep["name"] == dependency && dep["kind"].is_null())
        .unwrap_or_else(|| panic!("{package} names {dependency}"))
        .clone()
}

#[test]
fn the_facade_matrix_dependency_is_optional() {
    let matrix_dep = normal_dependency("powerio", "powerio-matrix");
    assert_eq!(matrix_dep["optional"], true, "matrix must be feature gated");
}

/// SQLite enters the graph only through `powerio-sincal`. Every edge to it is
/// optional and off without the `sincal` feature, so a facade built with
/// `default-features = false` needs no C compiler; the network crates' own
/// defaults are not re-enabled by the crates above them.
#[test]
fn every_powerio_sincal_dependency_is_optional() {
    for package in ["powerio", "powerio-tx", "powerio-dist"] {
        let dep = normal_dependency(package, "powerio-sincal");
        assert_eq!(
            dep["optional"], true,
            "{package}: sincal must be feature gated"
        );
    }
    for (package, network) in [
        ("powerio", "powerio-tx"),
        ("powerio", "powerio-dist"),
        ("powerio-prob", "powerio-tx"),
        ("powerio-prob", "powerio-dist"),
        ("powerio-matrix", "powerio-tx"),
        ("powerio-matrix", "powerio-dist"),
    ] {
        let dep = normal_dependency(package, network);
        assert_eq!(
            dep["uses_default_features"], false,
            "{package} -> {network} must not re-enable the default sincal feature"
        );
    }
}

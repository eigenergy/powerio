//! Windows-authored DSS decks in portable, case-sensitive memory inventories.
use std::sync::Arc;

use powerio_core::{ArtifactPath, Source, SourceTree, SourceTreeLimits};

fn parse(
    primary: &str,
    files: &[(&str, &str)],
) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    let limits = SourceTreeLimits::default();
    let tree = SourceTree::new(
        "deck",
        files.iter().map(|(name, bytes)| {
            (
                ArtifactPath::new(*name).unwrap(),
                Arc::<[u8]>::from(bytes.as_bytes()),
            )
        }),
        limits,
    )
    .unwrap();
    let primary = ArtifactPath::new(primary).unwrap();
    powerio_dist::parse(tree.source(None, Some(&primary), limits).unwrap()).unwrap()
}

#[test]
fn includes_resolve_unique_case_variants_and_report_original_dependencies() {
    let module = parse(
        "Master.DSS",
        &[
            ("Master.DSS", "new circuit.c\nredirect parts/loads.dss\n"),
            (
                "Parts/Loads.DSS",
                "redirect ../MORE.dss\nnew load.a bus1=sourcebus.1 phases=1 kv=1 kw=1\n",
            ),
            (
                "more.DSS",
                "new load.b bus1=sourcebus.2 phases=1 kv=1 kw=2\n",
            ),
        ],
    );
    assert_eq!(module.value().loads().len(), 2);
    assert!(
        !module
            .diagnostics()
            .iter()
            .any(|d| d.code().contains("INCLUDE"))
    );
    let names: Vec<_> = module.sources().iter().map(|s| s.id().as_str()).collect();
    assert!(names.contains(&"Parts/Loads.DSS"), "{names:?}");
    assert!(names.contains(&"more.DSS"), "{names:?}");
}

#[test]
fn exact_names_win_and_ambiguous_fallback_never_selects_a_file() {
    for (reference, expected) in [("Loads.DSS", 1), ("LOADS.DSS", 0)] {
        let master = format!("new circuit.c\nredirect {reference}\n");
        let module = parse(
            "master.dss",
            &[
                ("master.dss", &master),
                (
                    "Loads.DSS",
                    "new load.a bus1=sourcebus.1 phases=1 kv=1 kw=1\n",
                ),
                (
                    "loads.dss",
                    "new load.b bus1=sourcebus.2 phases=1 kv=1 kw=2\n",
                ),
            ],
        );
        assert_eq!(module.value().loads().len(), expected);
        assert_eq!(
            module
                .diagnostics()
                .iter()
                .any(|d| d.code() == "READ.DSS.INCLUDE_LOAD_FAILED"),
            expected == 0
        );
    }
}

#[test]
fn case_fallback_cannot_read_outside_selected_tree_root() {
    let limits = SourceTreeLimits::default();
    let tree = SourceTree::new(
        "deck",
        [
            ("case/Master.DSS", "new circuit.c\nredirect OUTSIDE.DSS\n"),
            (
                "Outside.DSS",
                "new load.a bus1=sourcebus.1 phases=1 kv=1 kw=1\n",
            ),
        ]
        .into_iter()
        .map(|(name, bytes)| {
            (
                ArtifactPath::new(name).unwrap(),
                Arc::<[u8]>::from(bytes.as_bytes()),
            )
        }),
        limits,
    )
    .unwrap();
    let source = tree
        .source(
            Some(&ArtifactPath::new("case").unwrap()),
            Some(&ArtifactPath::new("Master.DSS").unwrap()),
            limits,
        )
        .unwrap();
    let module = powerio_dist::parse(source.clone()).unwrap();
    assert_eq!(module.value().loads().len(), 0);
    assert!(source.dependencies().is_empty());
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DSS.INCLUDE_LOAD_FAILED")
    );
}

#[test]
fn named_companion_buffers_resolve_unique_case_variants() {
    // The Python and C entry points supply companions this way.
    let source = Source::from_memory(
        "Master.DSS",
        b"new circuit.c\nredirect loads.dss\n".to_vec(),
    )
    .unwrap()
    .with_named_buffer(
        "Loads.DSS",
        b"new load.a bus1=sourcebus.1 phases=1 kv=1 kw=1\n".to_vec(),
    )
    .unwrap();
    let module = powerio_dist::parse(source).unwrap();
    assert_eq!(module.value().loads().len(), 1);
    let names: Vec<_> = module.sources().iter().map(|s| s.id().as_str()).collect();
    assert!(names.contains(&"Loads.DSS"), "{names:?}");
}

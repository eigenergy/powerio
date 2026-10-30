use std::sync::Arc;

use powerio_core::{ArtifactPath, Source, SourceTree, SourceTreeLimits};

fn path(name: &str) -> ArtifactPath {
    ArtifactPath::new(name).unwrap()
}

fn tree(files: &[(&str, &[u8])]) -> SourceTree {
    SourceTree::new(
        "study",
        files
            .iter()
            .map(|(name, bytes)| (path(name), Arc::<[u8]>::from(*bytes))),
        SourceTreeLimits::default(),
    )
    .unwrap()
}

fn view(tree: &SourceTree, root: Option<&str>, primary: Option<&str>) -> Source {
    tree.source(
        root.map(path).as_ref(),
        primary.map(path).as_ref(),
        SourceTreeLimits::default(),
    )
    .unwrap()
}

fn dependencies(source: &Source) -> Vec<String> {
    source
        .dependencies()
        .iter()
        .map(|buffer| buffer.name().to_owned())
        .collect()
}

#[test]
fn selections_share_bytes_and_keep_observations_independent() {
    let shared = Arc::<[u8]>::from(&b"shared topology"[..]);
    let tree = SourceTree::new(
        "study",
        [
            (path("masters/a.dss"), Arc::from(&b"a"[..])),
            (path("masters/b.dss"), Arc::from(&b"b"[..])),
            (path("shared/topology.inc"), Arc::clone(&shared)),
        ],
        SourceTreeLimits::default(),
    )
    .unwrap();
    let a = view(&tree, None, Some("masters/a.dss"));
    let b = view(&tree, None, Some("masters/b.dss"));
    let a_primary = a.primary_buffer().unwrap();
    assert_eq!(a_primary.id().as_str(), "/input");
    assert_eq!(
        a_primary.directory_segments().collect::<Vec<_>>(),
        ["masters"]
    );
    assert!(dependencies(&a).is_empty());
    assert!(dependencies(&b).is_empty());
    let a_shared = a
        .referenced_buffer(&a_primary, "../shared/topology.inc")
        .unwrap();
    assert!(Arc::ptr_eq(&a_shared.shared_bytes(), &shared));
    assert_eq!(dependencies(&a), ["shared/topology.inc"]);
    assert!(dependencies(&b).is_empty());
    let b_shared = b.root_buffer("shared/topology.inc").unwrap();
    assert!(Arc::ptr_eq(
        &a_shared.shared_bytes(),
        &b_shared.shared_bytes()
    ));
    assert_eq!(dependencies(&b), ["shared/topology.inc"]);
    // The other selection's primary is an ordinary, distinct companion here.
    assert_eq!(
        a.root_buffer("masters/b.dss").unwrap().id().as_str(),
        "masters/b.dss"
    );
}

#[test]
fn directory_views_rebase_names_references_and_retained_companions() {
    let tree = tree(&[
        ("one/network.csv", b"network"),
        ("one/sub/loads.csv", b"loads"),
        ("two/network.csv", b"other"),
    ]);
    let source = view(&tree, Some("one"), None);
    assert!(source.is_directory());
    assert!(source.primary_buffer().is_err());
    assert_eq!(
        source.entry_names().unwrap(),
        [path("network.csv"), path("sub/loads.csv")]
    );
    assert_eq!(source.acquired_buffers().len(), 2);
    assert!(dependencies(&source).is_empty());
    let loads = source.buffer(&path("sub/loads.csv")).unwrap();
    let network = source.referenced_buffer(&loads, "../network.csv").unwrap();
    assert_eq!(network.name(), "network.csv");
    assert_eq!(network.id().as_str(), "network.csv");
    assert_eq!(dependencies(&source), ["network.csv", "sub/loads.csv"]);
    assert!(source.root_buffer("../two/network.csv").is_err());
    assert!(source.root_buffer("two/network.csv").is_err());
    assert_eq!(dependencies(&source).len(), 2);
}

#[test]
fn primary_is_relative_to_root_and_cannot_escape_it() {
    let tree = tree(&[
        ("one/model/main.dss", b"main"),
        ("one/data.bin", b"\0\xff"),
        ("outside.dss", b"outside"),
    ]);
    let source = view(&tree, Some("one"), Some("model/main.dss"));
    assert_eq!(source.name(), "model/main.dss");
    let primary = source.primary_buffer().unwrap();
    assert_eq!(
        source
            .referenced_buffer(&primary, "../data.bin")
            .unwrap()
            .bytes(),
        b"\0\xff"
    );
    assert!(
        source
            .referenced_buffer(&primary, "../../outside.dss")
            .is_err()
    );
    assert!(source.root_buffer("/outside.dss").is_err());
    assert!(source.root_buffer("C:\\outside.dss").is_err());
    assert!(source.with_acquisition_root("/").is_err());
}

#[test]
fn refused_reads_do_not_observe_dependencies_even_after_a_success() {
    let source = view(
        &tree(&[("main.dss", b"main"), ("data.bin", b"1234")]),
        None,
        Some("main.dss"),
    );
    let primary = source.primary_buffer().unwrap();
    assert!(
        source
            .referenced_buffer_with_limit(&primary, "data.bin", 3)
            .is_err()
    );
    assert!(source.referenced_buffer(&primary, "missing").is_err());
    assert!(dependencies(&source).is_empty());
    let data = source
        .referenced_buffer_with_limit(&primary, "data.bin", 4)
        .unwrap();
    assert!(
        source
            .referenced_buffer_with_limit(&primary, "data.bin", 3)
            .is_err()
    );
    assert_eq!(dependencies(&source), ["data.bin"]);
    assert_eq!(data.bytes(), b"1234");
}

#[test]
fn cloned_sources_share_observations_but_foreign_referrers_are_refused() {
    let tree = tree(&[("main.dss", b"main"), ("data", b"data")]);
    let source = view(&tree, None, Some("main.dss"));
    let clone = source.clone();
    std::thread::spawn(move || clone.root_buffer("data").unwrap())
        .join()
        .unwrap();
    assert_eq!(dependencies(&source), ["data"]);
    let other = view(&tree, None, Some("main.dss"));
    assert!(
        other
            .referenced_buffer(&source.primary_buffer().unwrap(), "data")
            .is_err()
    );
    assert!(dependencies(&other).is_empty());
}

#[test]
fn explicit_tree_and_view_limits_enforce_exact_boundaries() {
    let limits = SourceTreeLimits::new(2, 5, 2).unwrap();
    let files = || {
        [
            (path("one/a"), Arc::<[u8]>::from(&b"12"[..])),
            (path("one/b"), Arc::<[u8]>::from(&b"345"[..])),
        ]
    };
    let tree = SourceTree::new("study", files(), limits).unwrap();
    assert_eq!(limits.max_files(), 2);
    assert_eq!(limits.max_bytes(), 5);
    assert_eq!(limits.max_depth(), 2);
    let source = tree
        .source(
            Some(&path("one")),
            None,
            SourceTreeLimits::new(2, 5, 1).unwrap(),
        )
        .unwrap();
    assert_eq!(source.acquisition_byte_limit().unwrap(), 5);
    assert!(
        tree.source(None, None, SourceTreeLimits::new(1, 5, 2).unwrap())
            .is_err()
    );
    assert!(
        tree.source(None, None, SourceTreeLimits::new(2, 4, 2).unwrap())
            .is_err()
    );
    assert!(
        tree.source(None, None, SourceTreeLimits::new(2, 5, 1).unwrap())
            .is_err()
    );
    assert!(SourceTree::new("study", files(), SourceTreeLimits::new(1, 5, 2).unwrap()).is_err());
    assert!(SourceTree::new("study", files(), SourceTreeLimits::new(2, 4, 2).unwrap()).is_err());
    assert!(SourceTree::new("study", files(), SourceTreeLimits::new(2, 5, 1).unwrap()).is_err());
}

#[test]
fn invalid_limits_and_colliding_or_missing_paths_fail() {
    for (files, bytes, depth) in [
        (0, 1, 1),
        (1, 0, 1),
        (1, 1, 0),
        (1, 1, 65),
        (1, u64::MAX, 1),
    ] {
        assert!(SourceTreeLimits::new(files, bytes, depth).is_err());
    }
    for paths in [
        ["file", "file"],
        ["file", "file/child"],
        ["file/child", "file"],
    ] {
        assert!(
            SourceTree::new(
                "study",
                paths.map(|p| (path(p), Arc::<[u8]>::from(&b""[..]))),
                SourceTreeLimits::default()
            )
            .is_err()
        );
    }
    let tree = tree(&[("main.dss", b"main")]);
    assert!(
        tree.source(Some(&path("missing")), None, SourceTreeLimits::default())
            .is_err()
    );
    assert!(
        tree.source(Some(&path("main.dss")), None, SourceTreeLimits::default())
            .is_err()
    );
    assert!(
        tree.source(None, Some(&path("missing")), SourceTreeLimits::default())
            .is_err()
    );
}

#[test]
fn retained_inventory_is_complete_and_does_not_count_as_reads() {
    let source = view(
        &tree(&[
            ("a", b"a"),
            ("b", b"b"),
            ("main.dss", b"main"),
            ("notes", b"notes"),
        ]),
        None,
        Some("main.dss"),
    );
    assert_eq!(
        source
            .acquired_buffers()
            .iter()
            .map(|buffer| buffer.name().to_owned())
            .collect::<Vec<_>>(),
        ["main.dss", "a", "b", "notes"]
    );
    source.entry_names().unwrap();
    source.root_buffer("main.dss").unwrap();
    assert!(dependencies(&source).is_empty());
}

#[test]
fn legacy_memory_sources_distinguish_retained_from_read_companions() {
    let source = Source::from_memory("main.dss", b"main".as_slice())
        .unwrap()
        .with_named_buffer("data", b"data".as_slice())
        .unwrap();
    assert_eq!(source.acquired_buffers().len(), 2);
    assert!(dependencies(&source).is_empty());
    source
        .referenced_buffer(&source.primary_buffer().unwrap(), "data")
        .unwrap();
    assert_eq!(dependencies(&source), ["data"]);
}

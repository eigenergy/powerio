use powerio::collection::{ArchiveLimits, Cancellation};
use powerio::{
    ArtifactPath, Collection, CollectionLimits, CollectionSelection, ParseOptions, SourceTreeLimits,
};
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
fn zip(files: &[(&str, &[u8])]) -> Arc<[u8]> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zip.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner().into()
}
#[test]
fn nested_namespaces_and_native_layers_are_explicit() {
    let native = zip(&[("network.xml", b"<not-a-network/>")]);
    let child = zip(&[
        (
            "master.dss",
            b"New Circuit.child basekv=12.47\nRedirect missing.inc\n",
        ),
        ("native.zip", &native),
    ]);
    let outer = zip(&[
        ("a.zip", &child),
        ("b.zip", &child),
        ("missing.inc", b"New Circuit.parent"),
    ]);
    let collection = Collection::from_zip("outer.zip", outer, CollectionLimits::default()).unwrap();
    let a = collection.open_nested("a.zip").unwrap();
    let b = collection.open_nested("b.zip").unwrap();
    for child in [&a, &b] {
        let result = child
            .parse(
                &CollectionSelection::file("master.dss").format("dss"),
                &ParseOptions::default(),
            )
            .unwrap();
        assert!(result.incomplete);
        assert_eq!(result.dependencies, [] as [String; 0]);
        assert!(
            child
                .open_nested("native.zip")
                .unwrap_err()
                .to_string()
                .contains("layer limit")
        );
        assert!(
            child
                .parse(
                    &CollectionSelection::file("native.zip").format("cgmes"),
                    &ParseOptions::default()
                )
                .unwrap_err()
                .to_string()
                .contains("layer limit")
        );
    }
}
#[test]
fn siblings_clones_and_repeat_opens_share_expansion_and_entry_budgets() {
    let child = zip(&[("one.txt", b"abc"), ("two.txt", b"def")]);
    let outer = zip(&[("a.zip", &child), ("b.zip", &child)]);
    let mut limits = CollectionLimits::default();
    limits.tree = SourceTreeLimits::new(5, 4096, 64).unwrap();
    limits.archive = ArchiveLimits::new(4096, 5, 4096, 200).unwrap();
    let collection = Collection::from_zip("outer", outer, limits).unwrap();
    let cloned = collection.clone();
    collection.open_nested("a.zip").unwrap();
    assert!(cloned.open_nested("b.zip").is_err());
    assert!(cloned.open_nested("a.zip").is_err());

    let mut limits = CollectionLimits::default();
    limits.tree = SourceTreeLimits::new(4096, child.len() as u64 + 5, 64).unwrap();
    limits.archive = ArchiveLimits::new(4096, 4096, 4096, 200).unwrap();
    let collection = Collection::from_files(
        "memory",
        [(ArtifactPath::new("child.zip").unwrap(), child)],
        limits,
    )
    .unwrap();
    assert!(collection.open_nested("child.zip").is_err());
}
#[test]
fn work_is_bounded_per_operation_and_cancellation_is_sticky() {
    let bytes: Arc<[u8]> = Arc::from(&b"New Circuit.work basekv=12.47\n"[..]);
    let with_budget = |work_bytes| {
        Collection::from_files(
            "memory",
            [(ArtifactPath::new("master.dss").unwrap(), Arc::clone(&bytes))],
            {
                let mut limits = CollectionLimits::default();
                limits.work_bytes = work_bytes;
                limits
            },
        )
        .unwrap()
    };
    let selected = CollectionSelection::file("master.dss").format("dss");
    let collection = with_budget(bytes.len() as u64);
    // Each parse is charged on its own: a batch run or a converter session
    // parsing the same selection again never fails because of earlier work.
    collection
        .parse(&selected, &ParseOptions::default())
        .unwrap();
    collection
        .parse(&selected, &ParseOptions::default())
        .unwrap();
    assert!(
        with_budget(bytes.len() as u64 - 1)
            .parse(&selected, &ParseOptions::default())
            .unwrap_err()
            .to_string()
            .contains("work limit")
    );
    collection.cancellation().cancel();
    assert!(collection.inspect().is_err());
    assert!(
        collection
            .parse(&selected, &ParseOptions::default())
            .is_err()
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        Collection::from_zip_cancellable(
            "cancelled",
            zip(&[("x", b"a")]),
            CollectionLimits::default(),
            cancel
        )
        .is_err()
    );
}

#[test]
fn failed_verification_releases_its_reservation_and_inspection_is_bounded_per_call() {
    let mut corrupt = zip(&[("payload.xml", b"abcdef")]).to_vec();
    corrupt[30 + "payload.xml".len()] ^= 1;
    let mut limits = CollectionLimits::default();
    limits.tree = SourceTreeLimits::new(2, 4096, 64).unwrap();
    limits.archive = ArchiveLimits::new(4096, 2, 4096, 200).unwrap();
    for native in [false, true] {
        let collection = Collection::from_files(
            "memory",
            [(
                ArtifactPath::new("bad.zip").unwrap(),
                Arc::<[u8]>::from(corrupt.clone()),
            )],
            limits,
        )
        .unwrap();
        // A corrupt member is refused the same way every time: nothing of it
        // is retained, so the failed attempt holds no session allowance.
        let attempt = || {
            if native {
                let selected = CollectionSelection::file("bad.zip").format("cgmes");
                collection
                    .parse(&selected, &ParseOptions::default())
                    .map(|_| ())
            } else {
                collection.open_nested("bad.zip").map(|_| ())
            }
            .unwrap_err()
            .to_string()
        };
        let first = attempt();
        let second = attempt();
        assert_eq!(first, second);
        assert!(!second.contains("budget exhausted"), "{second}");
    }
    let with_budget = |work_bytes| {
        Collection::from_files(
            "memory",
            [(
                ArtifactPath::new("x").unwrap(),
                Arc::<[u8]>::from(&b"a"[..]),
            )],
            {
                let mut limits = CollectionLimits::default();
                limits.work_bytes = work_bytes;
                limits
            },
        )
        .unwrap()
    };
    let collection = with_budget(4);
    collection.inspect().unwrap();
    collection.inspect().unwrap();
    collection.inspect().unwrap();
    assert!(
        with_budget(1)
            .inspect()
            .unwrap_err()
            .to_string()
            .contains("work limit")
    );
}

#[test]
fn native_prefixed_zip_cannot_bypass_layers_and_dss_companions_are_opaque() {
    let native = zip(&[("network.xml", b"<not-a-network/>")]);
    let prefixed: Vec<u8> = b"prefix".iter().chain(native.iter()).copied().collect();
    let outer = zip(&[
        ("native.zip", &prefixed),
        ("native.sinx", &prefixed),
        ("Master.DSS", b"New Circuit.case basekv=12.47\n"),
    ]);
    let mut limits = CollectionLimits::default();
    limits.max_zip_layers = 1;
    let c = Collection::from_zip("outer", outer, limits).unwrap();
    for (name, format) in [("native.zip", "CGMES"), ("native.sinx", "sincal-balanced")] {
        let error = c
            .parse(
                &CollectionSelection::file(name).format(format),
                &ParseOptions::default(),
            )
            .unwrap_err();
        assert!(error.to_string().contains("layer limit"), "{error}");
    }
    for format in [None, Some("opendss")] {
        let mut selected = CollectionSelection::file("Master.DSS");
        selected.format = format.map(str::to_owned);
        assert!(c.parse(&selected, &ParseOptions::default()).is_ok());
    }
}

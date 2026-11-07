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
    let limits = CollectionLimits {
        tree: SourceTreeLimits::new(5, 4096, 64).unwrap(),
        archive: ArchiveLimits::new(4096, 5, 4096, 200).unwrap(),
        ..CollectionLimits::default()
    };
    let collection = Collection::from_zip("outer", outer, limits).unwrap();
    let cloned = collection.clone();
    collection.open_nested("a.zip").unwrap();
    assert!(cloned.open_nested("b.zip").is_err());
    assert!(cloned.open_nested("a.zip").is_err());

    let limits = CollectionLimits {
        tree: SourceTreeLimits::new(4096, child.len() as u64 + 5, 64).unwrap(),
        archive: ArchiveLimits::new(4096, 4096, 4096, 200).unwrap(),
        ..CollectionLimits::default()
    };
    let collection = Collection::from_files(
        "memory",
        [(ArtifactPath::new("child.zip").unwrap(), child)],
        limits,
    )
    .unwrap();
    assert!(collection.open_nested("child.zip").is_err());
}
#[test]
fn work_and_cancellation_do_not_reset_per_selection() {
    let bytes: Arc<[u8]> = Arc::from(&b"New Circuit.work basekv=12.47\n"[..]);
    let limits = CollectionLimits {
        work_bytes: bytes.len() as u64,
        ..CollectionLimits::default()
    };
    let collection = Collection::from_files(
        "memory",
        [(ArtifactPath::new("master.dss").unwrap(), bytes)],
        limits,
    )
    .unwrap();
    let selected = CollectionSelection::file("master.dss").format("dss");
    collection
        .parse(&selected, &ParseOptions::default())
        .unwrap();
    assert!(
        collection
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
fn failed_crc_reads_and_repeated_inspection_consume_session_allowances() {
    let mut corrupt = zip(&[("payload.xml", b"abcdef")]).to_vec();
    corrupt[30 + "payload.xml".len()] ^= 1;
    let limits = CollectionLimits {
        tree: SourceTreeLimits::new(2, 4096, 64).unwrap(),
        archive: ArchiveLimits::new(4096, 2, 4096, 200).unwrap(),
        ..CollectionLimits::default()
    };
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
        if native {
            let selected = CollectionSelection::file("bad.zip").format("cgmes");
            assert!(
                collection
                    .parse(&selected, &ParseOptions::default())
                    .is_err()
            );
            assert!(
                collection
                    .parse(&selected, &ParseOptions::default())
                    .unwrap_err()
                    .to_string()
                    .contains("budget exhausted")
            );
        } else {
            assert!(collection.open_nested("bad.zip").is_err());
            assert!(
                collection
                    .open_nested("bad.zip")
                    .unwrap_err()
                    .to_string()
                    .contains("budget exhausted")
            );
        }
    }
    let limits = CollectionLimits {
        work_bytes: 4,
        ..CollectionLimits::default()
    };
    let collection = Collection::from_files(
        "memory",
        [(
            ArtifactPath::new("x").unwrap(),
            Arc::<[u8]>::from(&b"a"[..]),
        )],
        limits,
    )
    .unwrap();
    collection.inspect().unwrap();
    collection.inspect().unwrap();
    assert!(
        collection
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
    let limits = CollectionLimits {
        max_zip_layers: 1,
        ..CollectionLimits::default()
    };
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

#![no_main]
use libfuzzer_sys::fuzz_target;
use powerio::{ArtifactPath, Collection, CollectionLimits, SourceTreeLimits};
use powerio_archive::{Archive, ArchiveLimits};
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 << 10 {
        return;
    }
    let archive = ArchiveLimits::new(64 << 10, 32, 128 << 10, 200).unwrap();
    let _ =
        powerio_archive::session::verify_native(data, archive, &Default::default(), |_, _| Ok(()));
    if let Ok(mut opened) = Archive::open(Arc::<[u8]>::from(data), archive) {
        let paths: Vec<_> = opened.entries().map(|e| e.path().clone()).collect();
        for path in paths {
            let first = opened.read(&path);
            let second = opened.read(&path);
            if let (Ok(a), Ok(b)) = (first, second) {
                assert!(Arc::ptr_eq(&a, &b));
            }
        }
    }
    let mut limits = CollectionLimits::default();
    limits.archive = archive;
    limits.tree = SourceTreeLimits::new(32, 128 << 10, 16).unwrap();
    limits.work_bytes = 512 << 10;
    if let Ok(collection) = Collection::from_zip("fuzz", Arc::from(data), limits) {
        let _ = collection.inspect();
        for entry in collection.entries().iter().take(4) {
            if entry.path.ends_with(".zip") {
                let _ = collection.open_nested(&entry.path);
            }
        }
    }
    let collection = Collection::from_files(
        "metadata",
        [(
            ArtifactPath::new("opaque.bin").unwrap(),
            Arc::<[u8]>::from(data),
        )],
        limits,
    )
    .unwrap();
    if let Ok(manifest) = collection.read_manifest(data) {
        let _ = manifest.to_json();
    }
    collection.cancellation().cancel();
    assert!(collection.inspect().is_err());
});

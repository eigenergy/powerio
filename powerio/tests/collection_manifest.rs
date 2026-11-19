use powerio::collection::{ArchiveLimits, CollectionCase, CollectionManifest, SourceBundleScope};
use powerio::{ArtifactPath, Collection, CollectionLimits, CollectionSelection, ParseOptions};
use std::sync::Arc;
fn files() -> Vec<(ArtifactPath, Arc<[u8]>)> {
    [
        (
            "masters/day.dss",
            &b"New Circuit.day basekv=12.47\nRedirect ../shared/line.inc\n"[..],
        ),
        (
            "masters/night.dss",
            &b"New Circuit.night basekv=12.47\nRedirect ../shared/line.inc\n"[..],
        ),
        (
            "shared/line.inc",
            &b"New Line.l bus1=source bus2=load r1=0.1 x1=0.2\n"[..],
        ),
        ("notes.bin", &b"\x00\xffopaque\r\n"[..]),
    ]
    .into_iter()
    .map(|(n, b)| (ArtifactPath::new(n).unwrap(), Arc::from(b)))
    .collect()
}
fn memory() -> Collection {
    Collection::from_files("study!literal.zip", files(), CollectionLimits::default()).unwrap()
}
fn case(id: &str) -> CollectionCase {
    let mut case = CollectionCase::new(
        id,
        CollectionSelection::file(format!("masters/{id}.dss")).format("dss"),
    );
    case.attachments = vec!["notes.bin".into()];
    case
}
#[test]
fn manifest_reproduces_independent_selections_and_rejects_untrusted_controls() {
    let collection = memory();
    let encoded = CollectionManifest::new(vec![case("day"), case("night")])
        .to_json()
        .unwrap();
    let manifest = collection.read_manifest(&encoded).unwrap();
    for id in ["night", "day"] {
        let result = collection.parse_manifest_case(&manifest, id).unwrap();
        let direct = collection
            .parse(&case(id).selection, &ParseOptions::default())
            .unwrap();
        assert_eq!(result.dependencies, direct.dependencies);
        assert_eq!(result.incomplete, direct.incomplete);
        let powerio::PioValue::MulticonductorNetwork(a) = result.module.value() else {
            panic!()
        };
        let powerio::PioValue::MulticonductorNetwork(b) = direct.module.value() else {
            panic!()
        };
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap()
        );
    }
    assert!(
        collection
            .read_manifest(
                br#"{"schema":"powerio-collection","version":1,"cases":[],"max_bytes":999999999}"#
            )
            .is_err()
    );
    assert!(
        collection
            .read_manifest(
                &CollectionManifest::new(vec![case("day"), case("day")])
                    .to_json()
                    .unwrap()
            )
            .is_err()
    );
    let mut bad = case("day");
    bad.selection.primary = Some("../outside".into());
    assert!(
        collection
            .read_manifest(&CollectionManifest::new(vec![bad]).to_json().unwrap())
            .is_err()
    );
    let mut bad = case("day");
    bad.attachments = vec!["absent".into()];
    assert!(
        collection
            .read_manifest(&CollectionManifest::new(vec![bad]).to_json().unwrap())
            .is_err()
    );
    assert!(
        collection
            .read_manifest(&vec![b' '; (1 << 20) + 1])
            .is_err()
    );
}
#[test]
fn bundle_scope_is_explicit_member_bytes_are_shared_and_packaging_is_deterministic() {
    let collection = memory();
    let selection = case("day").selection;
    let leaf = collection
        .source_bundle(&selection, SourceBundleScope::SelectedMembers)
        .unwrap();
    assert_eq!(leaf.files().len(), 1);
    assert_eq!(leaf.files()[0].0.as_str(), "masters/day.dss");
    let names = vec![
        "masters/day.dss".into(),
        "shared/line.inc".into(),
        "notes.bin".into(),
    ];
    let bundle = collection
        .source_bundle(&selection, SourceBundleScope::ExplicitMembers(names))
        .unwrap();
    assert_eq!(bundle.files().len(), 3);
    assert!(Arc::ptr_eq(&leaf.files()[0].1, &bundle.files()[0].1));
    let project = collection
        .source_bundle(&selection, SourceBundleScope::Project)
        .unwrap();
    assert_eq!(project.files().len(), 4);
    let mut reversed = bundle.files().to_vec();
    reversed.reverse();
    let bytes = bundle.deterministic_zip(ArchiveLimits::default()).unwrap();
    assert_eq!(
        bytes,
        powerio_archive::package::deterministic_zip(&reversed, ArchiveLimits::default()).unwrap()
    );
    drop(collection);
    let reopened =
        Collection::from_zip("rebuilt", bytes.into(), CollectionLimits::default()).unwrap();
    assert_eq!(reopened.entries().len(), 3);
    assert!(
        !reopened
            .parse(&selection, &ParseOptions::default())
            .unwrap()
            .incomplete
    );
    let restored = reopened
        .source_bundle(&selection, SourceBundleScope::Project)
        .unwrap();
    assert_eq!(restored.files(), bundle.files());
    assert!(
        bundle
            .deterministic_zip(ArchiveLimits::new(10, 10, 4096, 200).unwrap())
            .is_err()
    );
}
#[test]
fn original_zip_and_structured_nested_origin_are_distinct_from_rebuilt_packages() {
    let child =
        powerio_archive::package::deterministic_zip(&files(), ArchiveLimits::default()).unwrap();
    let outer = powerio_archive::package::deterministic_zip(
        &[(ArtifactPath::new("child!name.zip").unwrap(), child.into())],
        ArchiveLimits::default(),
    )
    .unwrap();
    let mut original = outer;
    // The original container may have a comment; rebuilding does not preserve it.
    let end = original.len();
    original[end - 2..].copy_from_slice(&7u16.to_le_bytes());
    original.extend_from_slice(b"comment");
    let collection = Collection::from_zip(
        "outer!name.zip",
        Arc::<[u8]>::from(original.clone()),
        CollectionLimits::default(),
    )
    .unwrap();
    assert_eq!(&*collection.original_zip().unwrap(), original);
    let mut selected = case("day");
    selected.containers = vec!["child!name.zip".into()];
    let manifest = collection
        .read_manifest(&CollectionManifest::new(vec![selected]).to_json().unwrap())
        .unwrap();
    assert!(
        !collection
            .parse_manifest_case(&manifest, "day")
            .unwrap()
            .incomplete
    );
    let child = collection.open_nested("child!name.zip").unwrap();
    let origin = child.selection_origin(&case("day").selection);
    assert_eq!(origin.origin.collection, "outer!name.zip");
    assert_eq!(origin.origin.containers, ["child!name.zip"]);
    assert_eq!(origin.selection.primary.as_deref(), Some("masters/day.dss"));
}
#[test]
fn package_rejects_portable_collisions_and_duplicates() {
    for names in [["a", "A"], ["a", "a"], ["a", "a/b"], ["con", "okay"]] {
        let files = names.map(|p| (ArtifactPath::new(p).unwrap(), Arc::<[u8]>::from(&b"x"[..])));
        assert!(
            powerio_archive::package::deterministic_zip(&files, ArchiveLimits::default()).is_err()
        );
    }
}

#[test]
fn manifest_reads_are_bounded_per_call_and_direct_catalogs_obey_host_budgets() {
    let bytes = br#"{"schema":"powerio-collection","version":1,"cases":[]}"#;
    let with_budget = |work_bytes| {
        Collection::from_files("empty", [], {
            let mut limits = CollectionLimits::default();
            limits.work_bytes = work_bytes;
            limits
        })
        .unwrap()
    };
    let collection = with_budget(bytes.len() as u64);
    collection.read_manifest(bytes).unwrap();
    collection.read_manifest(bytes).unwrap();
    assert!(
        with_budget(bytes.len() as u64 - 1)
            .read_manifest(bytes)
            .unwrap_err()
            .to_string()
            .contains("work limit")
    );
    let collection = Collection::from_files("empty", [], {
        let mut limits = CollectionLimits::default();
        limits.inspection_bytes = 1;
        limits
    })
    .unwrap();
    assert!(
        collection
            .parse_manifest_case(&CollectionManifest::new(vec![]), "missing")
            .unwrap_err()
            .to_string()
            .contains("metadata limit")
    );
    let mut manifest = CollectionManifest::new(vec![]);
    manifest.schema = "x".repeat((1 << 20) + 1);
    assert!(manifest.to_json().is_err());
}

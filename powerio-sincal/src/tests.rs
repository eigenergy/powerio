use std::{
    borrow::Cow,
    io::{Cursor, Write},
};

use powerio_core::Source;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{AcquiredProject, SQLITE_MAGIC, database_bytes};

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn transport_does_not_require_an_electrical_schema_or_model_family() {
    // Header recognition is transport only. This deliberately is not a valid
    // database and cannot be evidence of successful balanced/unbalanced parse.
    assert!(matches!(
        database_bytes(SQLITE_MAGIC).unwrap(),
        Cow::Borrowed(_)
    ));
    let source = Source::from_memory("input.db", SQLITE_MAGIC.to_vec())
        .unwrap()
        .with_named_buffer("opaque.bin", vec![0, 255])
        .unwrap();
    let acquired = AcquiredProject::read(&source).unwrap();
    assert_eq!(acquired.database.bytes(), SQLITE_MAGIC);
    assert_eq!(
        acquired.source.root_buffer("opaque.bin").unwrap().bytes(),
        [0, 255]
    );
    assert_eq!(source.acquired_buffers().len(), 2);
}

#[test]
fn package_namespace_and_original_bytes_are_retained_without_external_fallback() {
    let bytes = archive(&[
        (
            "SIArchive.ini",
            b"[Main]\nAppVersion=PSS SINCAL\nNetworkType=Electro\n",
        ),
        ("case_files/database.db", SQLITE_MAGIC),
        (
            "case_files/database.ini",
            b"FILE=C:\\unrelated\\database.db",
        ),
        ("case_files/opaque.bin", &[0, 255, 10]),
    ]);
    let source = Source::from_memory("case.sinx", bytes.clone())
        .unwrap()
        .with_named_buffer("caller-only.bin", vec![1])
        .unwrap();
    let acquired = AcquiredProject::read(&source).unwrap();
    assert_eq!(acquired.source.primary_buffer().unwrap().bytes(), bytes);
    assert_eq!(acquired.database.bytes(), SQLITE_MAGIC);
    assert_eq!(
        acquired
            .source
            .root_buffer("case_files/opaque.bin")
            .unwrap()
            .bytes(),
        [0, 255, 10]
    );
    assert!(acquired.source.root_buffer("caller-only.bin").is_err());
    assert_eq!(source.acquired_buffers().len(), 2);
}

#[test]
fn malformed_packages_remain_transport_errors() {
    let marker = b"[Main]\nAppVersion=PSS SINCAL\nNetworkType=Electro\n";
    for entries in [
        vec![
            ("SIArchive.ini", &marker[..]),
            ("../case_files/database.db", SQLITE_MAGIC),
        ],
        vec![
            ("SIArchive.ini", &marker[..]),
            ("a_files/database.db", SQLITE_MAGIC),
            ("b_files/database.db", SQLITE_MAGIC),
        ],
        vec![
            ("SIArchive.ini", &b"[Main]\nNetworkType=Gas\n"[..]),
            ("case_files/database.db", SQLITE_MAGIC),
        ],
    ] {
        let bytes = archive(&entries);
        assert!(database_bytes(&bytes).is_err());
        assert!(AcquiredProject::read(&Source::from_memory("case.sinx", bytes).unwrap()).is_err());
    }
}

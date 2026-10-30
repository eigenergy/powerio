use powerio_archive::{Archive, ArchiveLimits, codes};
use powerio_core::{ArtifactPath, DiagnosticInfo};
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn path(name: &str) -> ArtifactPath {
    ArtifactPath::new(name).unwrap()
}
fn open(bytes: Vec<u8>) -> Result<Archive, powerio_core::Error> {
    Archive::open(bytes, ArchiveLimits::default())
}
fn zip(files: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for &(name, bytes) in files {
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn at(bytes: &[u8], magic: &[u8]) -> usize {
    bytes.windows(4).position(|b| b == magic).unwrap()
}
fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn get32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn refused(bytes: Vec<u8>, code: &'static DiagnosticInfo) {
    let error = open(bytes).unwrap_err();
    assert_eq!(error.info().unwrap().code, code.code, "{error}");
}

#[test]
fn reads_preserve_bytes_order_and_shared_ownership() {
    assert!(powerio_core::check_registry(codes::ALL.iter().copied()).is_empty());
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let nested = zip(&[("uninterpreted.bin", b"opaque")], method);
        let bytes: Arc<[u8]> = zip(
            &[
                ("z.zip", &nested),
                ("é/a.dss", b"arbitrary\0bytes"),
                ("empty", b""),
            ],
            method,
        )
        .into();
        let mut archive = Archive::open(Arc::clone(&bytes), ArchiveLimits::default()).unwrap();
        assert!(Arc::ptr_eq(&bytes, &archive.original_bytes()));
        assert_eq!(
            archive
                .entries()
                .map(|e| e.path().as_str())
                .collect::<Vec<_>>(),
            ["empty", "z.zip", "é/a.dss"]
        );
        let first = archive.read(&path("z.zip")).unwrap();
        assert_eq!(&*first, nested);
        assert!(Arc::ptr_eq(&first, &archive.read(&path("z.zip")).unwrap()));
        assert!(archive.read_with_limit(&path("z.zip"), 1).is_err());
        assert_eq!(
            &*archive.read(&path("é/a.dss")).unwrap(),
            b"arbitrary\0bytes"
        );
        assert!(
            archive
                .read_with_limit(&path("empty"), 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            archive
                .read(&path("missing"))
                .unwrap_err()
                .info()
                .unwrap()
                .code,
            codes::UNKNOWN_ENTRY.code
        );
    }
    assert_eq!(
        open(zip(&[], CompressionMethod::Stored))
            .unwrap()
            .entries()
            .len(),
        0
    );
}

#[test]
fn explicit_budgets_apply_at_their_boundaries() {
    let bytes = zip(&[("a", b"123"), ("b", b"456")], CompressionMethod::Stored);
    let n = bytes.len() as u64;
    for limits in [(n - 1, 2, 6, 1), (n, 1, 6, 1), (n, 2, 5, 1)] {
        let limits = ArchiveLimits::new(limits.0, limits.1, limits.2, limits.3).unwrap();
        assert_eq!(
            Archive::open(bytes.clone(), limits)
                .unwrap_err()
                .info()
                .unwrap()
                .code,
            codes::LIMIT.code
        );
    }
    let mut archive = Archive::open(bytes, ArchiveLimits::new(n, 2, 6, 1).unwrap()).unwrap();
    assert!(archive.read_with_limit(&path("a"), 2).is_err());
    assert_eq!(&*archive.read_with_limit(&path("a"), 3).unwrap(), b"123");
    for values in [
        (0, 1, 1, 1),
        (1, 0, 1, 1),
        (1, 1, 0, 1),
        (1, 1, 1, 0),
        (u64::MAX, 1, 1, 1),
        (1, usize::MAX, 1, 1),
    ] {
        assert!(ArchiveLimits::new(values.0, values.1, values.2, values.3).is_err());
    }
    let bomb = zip(&[("bomb", &[0; 4096])], CompressionMethod::Deflated);
    let limits = ArchiveLimits::new(10000, 1, 4096, 1).unwrap();
    assert_eq!(
        Archive::open(bomb, limits)
            .unwrap_err()
            .info()
            .unwrap()
            .code,
        codes::LIMIT.code
    );
}

#[test]
fn rejects_nonportable_names_and_ancestor_collisions() {
    for name in [
        "../a",
        "/a",
        "a/../b",
        "a/./b",
        "a//b",
        "C:a",
        "a\\b",
        "a\0b",
        "con.txt",
        "x/AUX",
        "lpt1",
        "COM¹.txt",
        "a*",
        "a?",
        "a|",
        "a<",
        "a>",
        "a\"",
        "a.",
        "a ",
    ] {
        refused(
            zip(&[(name, b"x")], CompressionMethod::Stored),
            &codes::UNSAFE,
        );
    }
    for names in [["A", "a"], ["a", "a/b"], ["A/b", "a/c"], ["a/b", "a"]] {
        refused(
            zip(
                &[(names[0], b""), (names[1], b"")],
                CompressionMethod::Stored,
            ),
            &codes::UNSAFE,
        );
    }
    let deep = ["a"; 65].join("/");
    refused(
        zip(&[(&deep, b"")], CompressionMethod::Stored),
        &codes::LIMIT,
    );
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink("link", "target", SimpleFileOptions::default())
        .unwrap();
    refused(writer.finish().unwrap().into_inner(), &codes::UNSAFE);
}

#[test]
fn directories_and_comments_remain_transport_metadata() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_comment("archive comment").unwrap();
    writer
        .add_directory("dir/", SimpleFileOptions::default())
        .unwrap();
    writer
        .start_file("dir/file", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"data").unwrap();
    let mut archive = open(writer.finish().unwrap().into_inner()).unwrap();
    assert!(archive.entries().next().unwrap().is_directory());
    assert!(archive.read(&path("dir")).unwrap().is_empty());
    assert_eq!(&*archive.read(&path("dir/file")).unwrap(), b"data");
}

#[test]
fn deep_names_and_late_directory_entries_share_a_namespace() {
    let first = format!("root/{}", vec!["a".repeat(60); 63].join("/"));
    let second = first.replace("root/", "other/");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for name in [&first, &second] {
        writer
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"x").unwrap();
    }
    writer
        .add_directory("root/", SimpleFileOptions::default())
        .unwrap();
    let mut archive = open(writer.finish().unwrap().into_inner()).unwrap();
    assert_eq!(archive.entries().len(), 3);
    assert_eq!(&*archive.read(&path(&first)).unwrap(), b"x");
    assert_eq!(&*archive.read(&path(&second)).unwrap(), b"x");
    assert!(archive.read(&path("root")).unwrap().is_empty());
}

#[test]
fn corruption_is_lazy_and_never_cached() {
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let base = zip(&[("a", b"payload"), ("valid", b"still readable")], method);
        let central = at(&base, b"PK\x01\x02");
        for (local_offset, central_offset, value) in [(14, 16, 123), (22, 24, 6), (22, 24, 8)] {
            let mut bytes = base.clone();
            put32(&mut bytes, local_offset, value);
            put32(&mut bytes, central + central_offset, value);
            let mut archive = open(bytes).unwrap();
            for _ in 0..2 {
                assert!(archive.read(&path("a")).is_err());
            }
            assert_eq!(&*archive.read(&path("valid")).unwrap(), b"still readable");
        }
    }
}

#[test]
fn local_layout_and_transport_features_are_checked_before_reads() {
    let base = zip(
        &[("a", b"payload"), ("b", b"second")],
        CompressionMethod::Stored,
    );
    let central = at(&base, b"PK\x01\x02");
    for offset in [6, 8, 14, 18, 22, 26, 28, 30] {
        let mut bytes = base.clone();
        bytes[offset] ^= 1;
        assert!(open(bytes).is_err(), "local header byte {offset}");
    }
    for (flag, method) in [(1, 0), (64, 0), (8192, 0), (0, 99)] {
        let mut bytes = base.clone();
        put16(&mut bytes, 6, flag);
        put16(&mut bytes, central + 8, flag);
        put16(&mut bytes, 8, method);
        put16(&mut bytes, central + 10, method);
        assert!(open(bytes).is_err());
    }
    let mut overlap = base.clone();
    let size = (central - 31) as u32;
    for offset in [18, 22, central + 20, central + 24] {
        put32(&mut overlap, offset, size);
    }
    refused(overlap, &codes::UNSAFE);
    let mut special = base;
    put32(&mut special, central + 38, 0o060_000 << 16);
    refused(special, &codes::UNSAFE);
}

#[test]
fn central_directory_preflight_refuses_forged_and_ambiguous_metadata() {
    let base = zip(&[("a", b"payload")], CompressionMethod::Stored);
    let end = base.len() - 22;
    for offset in [
        end + 4,
        end + 6,
        end + 8,
        end + 10,
        end + 12,
        end + 16,
        end + 20,
    ] {
        let mut bytes = base.clone();
        bytes[offset] ^= 1;
        assert!(open(bytes).is_err(), "end record byte {offset}");
    }
    let mut too_many = base.clone();
    put16(&mut too_many, end + 8, 5000);
    put16(&mut too_many, end + 10, 5000);
    refused(too_many, &codes::LIMIT);
    let central = at(&base, b"PK\x01\x02");
    for offset in [
        central,
        central + 28,
        central + 30,
        central + 32,
        central + 34,
        central + 42,
    ] {
        let mut bytes = base.clone();
        bytes[offset] ^= 1;
        assert!(open(bytes).is_err(), "central record byte {offset}");
    }
    let mut ambiguous = base.clone();
    put16(&mut ambiguous, end + 20, 22);
    ambiguous.extend_from_slice(&base[end..]);
    refused(ambiguous, &codes::MALFORMED);
    let mut trailing = base.clone();
    trailing.push(0);
    refused(trailing, &codes::MALFORMED);
    for len in 0..base.len() {
        assert!(open(base[..len].to_vec()).is_err());
    }
}

#[test]
fn embedded_end_signatures_are_opaque_payloads() {
    let payload = b"PK\x05\x06PK\x06\x06PK\x06\x07".repeat(1000);
    let mut archive = open(zip(&[("nested.zip", &payload)], CompressionMethod::Stored)).unwrap();
    assert_eq!(&*archive.read(&path("nested.zip")).unwrap(), payload);
}

#[test]
fn bit_mutations_never_panic_while_indexing_or_reading() {
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let base = zip(&[("a", b"payload")], method);
        for at in 0..base.len() {
            for bit in 0..8 {
                let mut bytes = base.clone();
                bytes[at] ^= 1 << bit;
                let limits = ArchiveLimits::new(2048, 8, 4096, 200).unwrap();
                if let Ok(mut archive) = Archive::open(bytes, limits) {
                    let paths: Vec<_> = archive.entries().map(|e| e.path().clone()).collect();
                    for path in paths {
                        let _ = archive.read(&path);
                    }
                }
            }
        }
    }
}

#[test]
fn zip64_end_records_and_members_have_checked_bounds() {
    let base = zip(&[("a", b"payload")], CompressionMethod::Stored);
    let end = base.len() - 22;
    let mut bytes = base[..end].to_vec();
    bytes.extend_from_slice(b"PK\x06\x06");
    bytes.extend_from_slice(&44u64.to_le_bytes());
    bytes.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    for value in [
        1,
        1,
        u64::from(get32(&base, end + 12)),
        u64::from(get32(&base, end + 16)),
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(b"PK\x06\x07");
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(end as u64).to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&base[end..]);
    let mut archive = open(bytes.clone()).unwrap();
    assert_eq!(&*archive.read(&path("a")).unwrap(), b"payload");
    let mut saturated = bytes.clone();
    let legacy = saturated.len() - 22;
    for offset in [legacy + 8, legacy + 10] {
        put16(&mut saturated, offset, u16::MAX);
    }
    for offset in [legacy + 12, legacy + 16] {
        put32(&mut saturated, offset, u32::MAX);
    }
    assert_eq!(
        &*open(saturated.clone()).unwrap().read(&path("a")).unwrap(),
        b"payload"
    );
    let mut excessive = saturated;
    for offset in [end + 24, end + 32] {
        excessive[offset..offset + 8].copy_from_slice(&5000u64.to_le_bytes());
    }
    refused(excessive, &codes::LIMIT);
    for offset in [
        end + 4,
        end + 16,
        end + 24,
        end + 32,
        end + 40,
        end + 48,
        end + 64,
        end + 72,
    ] {
        let mut bad = bytes.clone();
        bad[offset] ^= 1;
        assert!(open(bad).is_err(), "ZIP64 footer byte {offset}");
    }
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("large", SimpleFileOptions::default().large_file(true))
        .unwrap();
    writer
        .write_all(b"small payload with ZIP64 local sizes")
        .unwrap();
    let mut archive = open(writer.finish().unwrap().into_inner()).unwrap();
    assert_eq!(
        &*archive.read(&path("large")).unwrap(),
        b"small payload with ZIP64 local sizes"
    );
}

#[test]
fn streaming_descriptors_are_verified_in_both_widths() {
    for large in [false, true] {
        let mut writer = ZipWriter::new_stream(Vec::new());
        writer
            .start_file("a", SimpleFileOptions::default().large_file(large))
            .unwrap();
        writer.write_all(b"stream").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let descriptor = at(&bytes, b"PK\x07\x08");
        for offset in [14, 18, 22] {
            let mut bad = bytes.clone();
            bad[offset] ^= 1;
            refused(bad, &codes::MALFORMED);
        }
        let mut archive = open(bytes.clone()).unwrap();
        assert_eq!(&*archive.read(&path("a")).unwrap(), b"stream");
        for offset in [
            descriptor + 4,
            descriptor + 8,
            descriptor + if large { 16 } else { 12 },
        ] {
            let mut bad = bytes.clone();
            bad[offset] ^= 1;
            refused(bad, &codes::MALFORMED);
        }
        let central = at(&bytes, b"PK\x01\x02");
        let mut unsigned = bytes;
        unsigned.drain(descriptor..descriptor + 4);
        let end = unsigned.len() - 22;
        put32(&mut unsigned, end + 16, (central - 4) as u32);
        let mut archive = open(unsigned).unwrap();
        assert_eq!(&*archive.read(&path("a")).unwrap(), b"stream");
    }
}

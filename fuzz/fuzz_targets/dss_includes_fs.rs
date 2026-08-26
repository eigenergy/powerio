//! Filesystem-backed include fuzzing for the OpenDSS reader (#339): the half
//! the in-memory `dss_includes` target cannot reach. Each input materializes
//! a small tree — a case root with a main script, two includes (one nested in
//! a subdirectory), a file outside the root, and on Unix a symbolic link
//! escaping the root — and parses through `Source::open`, so the descriptor
//! walk, canonical containment, and symlink refusal all execute against a
//! real filesystem. Filesystem churn per execution keeps this target slow;
//! it exists for coverage of the canonical half, and its throughput is
//! recorded in #421.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut parts = data.split(|b| *b == 0xFF);
    let Some(main) = parts.next() else {
        return;
    };
    if std::str::from_utf8(main).is_err() {
        return;
    }
    let inner = parts.next().unwrap_or(b"");
    let nested = parts.next().unwrap_or(b"");
    let outside = parts.next().unwrap_or(b"");

    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let root = dir.path().join("case");
    if std::fs::create_dir_all(root.join("sub")).is_err() {
        return;
    }
    let main_path = root.join("main.dss");
    if std::fs::write(&main_path, main).is_err()
        || std::fs::write(root.join("inc0.dss"), inner).is_err()
        || std::fs::write(root.join("sub").join("inc1.dss"), nested).is_err()
        || std::fs::write(dir.path().join("outside.dss"), outside).is_err()
    {
        return;
    }
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(dir.path().join("outside.dss"), root.join("link.dss"));

    let Ok(source) = powerio_core::Source::open(&main_path) else {
        return;
    };
    let Ok(id) = powerio_core::FormatId::new("dss") else {
        return;
    };
    let _ = powerio_dist::parse(source.with_format(id));
});

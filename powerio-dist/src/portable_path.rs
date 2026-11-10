//! Platform independent tests on include paths written in case files.
//!
//! A case file can be written on one operating system and read on another,
//! so whether an include names an absolute path must not depend on the host.
//! `std::path::Path::is_absolute` does: it rejects `C:\x` on Unix and `/x`
//! on Windows.

/// Whether `path`, as written in a case file, names an absolute location on
/// any common platform: a leading `/` or `\` (a Unix root, a Windows root
/// relative path, or a UNC path such as `\\server\share`), or a drive letter
/// followed by a separator (`C:\x` or `C:/x`).
pub(crate) fn is_portable_absolute(path: &str) -> bool {
    match path.as_bytes() {
        [b'/' | b'\\', ..] => true,
        [drive, b':', b'/' | b'\\', ..] => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_portable_absolute;

    #[test]
    fn unix_and_windows_absolute_paths_are_absolute_on_every_host() {
        for path in [
            "/feeder/load.glm",
            "/",
            r"\feeder\load.glm",
            r"\\server\share\load.glm",
            "//server/share/load.glm",
            r"C:\feeder\load.glm",
            "C:/feeder/load.glm",
            r"z:\load.dss",
        ] {
            assert!(is_portable_absolute(path), "{path}");
        }
    }

    #[test]
    fn relative_paths_are_not_absolute() {
        for path in [
            "",
            "load.glm",
            "sub/load.glm",
            r"sub\load.glm",
            "./load.glm",
            r"..\load.dss",
            "C:load.dss",
            "C:",
            "1:/load.dss",
            "file:/load.glm",
        ] {
            assert!(!is_portable_absolute(path), "{path}");
        }
    }
}

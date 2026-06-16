//! DIgSILENT PowerFactory `.pfd` project files.
//!
//! `.pfd` is DIgSILENT's encrypted binary project export. The payload is
//! statistically a cipher stream (uniform byte distribution, no compression or
//! container structure, no recoverable record anchors), and no public decoder or
//! key exists: every tool that reads `.pfd` drives a licensed PowerFactory
//! runtime. powerio therefore rejects `.pfd` rather than fabricating a partial
//! [`Network`]. To read PowerFactory data, export the project as DGS and use
//! [`super::dgs`], the plaintext interchange reader.
//!
//! [`Network`]: crate::network::Network

use crate::network::Network;
use crate::{Error, Result};

const FMT: &str = "PowerFactory .pfd";

/// Parse a DIgSILENT PowerFactory `.pfd` binary project.
///
/// # Errors
/// Always returns [`Error::FormatRead`]: `.pfd` is an encrypted binary project
/// export with no public decoder, so no power flow tables can be recovered
/// without a licensed PowerFactory runtime. Export the project as DGS instead.
pub fn parse_powerfactory_pfd(bytes: &[u8], _name_hint: Option<&str>) -> Result<Network> {
    Err(classification_error(bytes))
}

fn classification_error(bytes: &[u8]) -> Error {
    let message = if bytes.is_empty() {
        "empty PowerFactory .pfd file".to_string()
    } else if let Some(kind) = known_container(bytes) {
        format!(
            "unsupported PowerFactory .pfd container: input starts with {kind}; \
             no PowerFactory project decoder is implemented"
        )
    } else {
        "unsupported DIgSILENT PowerFactory .pfd binary project: an encrypted \
         binary project export (uniform high entropy bytes, no file magic, \
         archive directory, or decodable table layout) with no public decoder. \
         Export the project as DGS from PowerFactory (File > Export > DGS) and \
         read the .dgs with powerio instead"
            .to_string()
    };
    Error::FormatRead {
        format: FMT,
        message,
    }
}

fn known_container(bytes: &[u8]) -> Option<&'static str> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    if starts(b"PK\x03\x04") {
        Some("a ZIP header")
    } else if starts(b"\x1f\x8b") {
        Some("a gzip header")
    } else if starts(b"\xfd7zXZ\0") {
        Some("an xz header")
    } else if starts(b"7z\xbc\xaf\x27\x1c") {
        Some("a 7z header")
    } else if starts(b"Rar!\x1a\x07") {
        Some("a RAR header")
    } else if starts(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") {
        Some("an OLE compound file header")
    } else if starts(b"SQLite format 3\0") {
        Some("a SQLite header")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::parse_powerfactory_pfd;

    #[test]
    fn opaque_bytes_reject_with_powerfactory_context() {
        let err = parse_powerfactory_pfd(&[0x47, 0x50, 0x0c, 0x2a, 0x07, 0x4b], None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("PowerFactory .pfd"), "{err}");
        assert!(err.contains("encrypted binary project export"), "{err}");
        assert!(err.contains("DGS"), "{err}");
    }

    #[test]
    fn known_container_headers_reject_with_the_container_named() {
        let err = parse_powerfactory_pfd(b"PK\x03\x04not-a-project", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ZIP"), "{err}");
    }
}

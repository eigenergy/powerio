//! PowerIO IR serialization and deserialization.

use powerio_core::{
    ArtifactPath, Diagnostic, EmitResult, Error, Fidelity, MemoryArtifact, PioModule,
};

use crate::PioValue;

/// Whether this build deserializes a PowerIO IR document stating `version`.
///
/// This is the one statement of the compatibility window that
/// [`IR_SCHEMA_VERSION`](crate::IR_SCHEMA_VERSION) documents: a document is
/// readable when a SemVer compatible build no newer than this one wrote it.
pub(crate) fn ir_version_is_readable(version: &str) -> bool {
    readable_by(version, crate::IR_SCHEMA_VERSION)
}

/// Whether a document stating `version` was written by a PowerIO release
/// later than this build, so that upgrading is what makes it readable. An
/// unreadable document that is not newer came from an earlier line and has to
/// be regenerated instead.
pub(crate) fn ir_version_is_newer(version: &str) -> bool {
    match (
        parse_version(version),
        parse_version(crate::IR_SCHEMA_VERSION),
    ) {
        (Some(document), Some(this)) => document > this,
        _ => false,
    }
}

/// A document written by `document` is readable by a build at `reader` when
/// the two versions are SemVer compatible and the reader is not the older.
fn readable_by(document: &str, reader: &str) -> bool {
    let (Some((major, minor, patch)), Some((reader_major, reader_minor, reader_patch))) =
        (parse_version(document), parse_version(reader))
    else {
        return false;
    };
    if major != reader_major {
        return false;
    }
    if major == 0 {
        minor == reader_minor && patch <= reader_patch
    } else {
        (minor, patch) <= (reader_minor, reader_patch)
    }
}

/// `MAJOR.MINOR.PATCH` as numbers. A prerelease, a build tag, or any other
/// spelling is not a released PowerIO version and reads as none.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    fn number(part: &str) -> Option<u64> {
        (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    }
    let mut parts = text.split('.');
    let version = (
        number(parts.next()?)?,
        number(parts.next()?)?,
        number(parts.next()?)?,
    );
    parts.next().is_none().then_some(version)
}

/// Serialize a diagnostics list as the JSON array of PowerIO IR diagnostic
/// records: the encoding a module's `diagnostics` field carries, with an
/// identity minted for every record that has none.
///
/// # Errors
/// The records cannot be encoded as JSON.
pub fn serialize_diagnostics(diagnostics: &[Diagnostic]) -> Result<String, Error> {
    let records = crate::stored::encode_diagnostics(diagnostics);
    serde_json::to_string(&records).map_err(|cause| {
        Error::new(
            &crate::codes::READ_MODULE_INVALID,
            format!("diagnostics could not be encoded as JSON: {cause}"),
        )
        .with_cause(cause)
    })
}

/// Generate the JSON Schema for this build's PowerIO IR document.
#[cfg(feature = "schema")]
#[must_use]
pub fn generate_ir_schema() -> schemars::Schema {
    schemars::schema_for!(crate::stored::StoredModule)
}

/// Serialize a dynamic module as PowerIO IR.
///
/// PowerIO IR is the durable representation of a complete [`PioModule`]. It
/// is separate from the grid exchange formats handled by [`crate::emit`].
///
/// # Errors
/// The module cannot be represented by the current IR schema, or the
/// destination refuses the artifact.
pub fn serialize<T>(
    module: &PioModule<T>,
    output: impl powerio_core::IntoDestination,
) -> Result<EmitResult, Error>
where
    T: Clone + Into<PioValue>,
{
    let module = module.clone().map_value(Into::into);
    let text = crate::stored::emit_module(&module)?;
    let artifact = MemoryArtifact::new(ArtifactPath::new("module.pio.json")?, text.into_bytes());
    output.into_destination()?.__commit_artifacts(
        false,
        Fidelity::Canonical,
        vec![artifact],
        Vec::new(),
    )
}

/// Deserialize one PowerIO IR input: a file name, content in memory, or a
/// [`powerio_core::Source`].
///
/// # Errors
/// The source is not one UTF-8 IR document, or its schema is unsupported or
/// invalid.
pub fn deserialize(input: impl powerio_core::IntoSource) -> Result<PioModule<PioValue>, Error> {
    let source = input.into_source()?;
    let buffer = source.primary_buffer()?;
    let text = std::str::from_utf8(buffer.content_bytes()).map_err(|cause| {
        Error::new(
            &crate::codes::READ_MODULE_INVALID,
            format!("PowerIO IR is not valid UTF-8: {cause}"),
        )
        .with_cause(cause)
        .with_source(source.clone())
    })?;
    crate::stored::read_module(text)
        .map(|module| module.with_source(source.clone()))
        .map_err(|error| error.with_source(source))
}

#[cfg(test)]
mod tests {
    use super::{ir_version_is_newer, ir_version_is_readable, readable_by};

    #[test]
    fn this_build_reads_its_own_documents_and_is_not_newer_than_itself() {
        assert!(ir_version_is_readable(crate::IR_SCHEMA_VERSION));
        assert!(!ir_version_is_newer(crate::IR_SCHEMA_VERSION));
    }

    /// A refusal advises an upgrade for every later release and a regenerated
    /// document for every earlier one, on this build's line or another.
    #[test]
    fn a_later_release_is_newer_whatever_line_it_is_on() {
        let (major, minor, patch) = super::parse_version(crate::IR_SCHEMA_VERSION).unwrap();
        for (m, n, p) in [
            (major, minor, patch + 1),
            (major, minor + 1, 0),
            (major + 1, 0, 0),
        ] {
            let later = format!("{m}.{n}.{p}");
            assert!(ir_version_is_newer(&later), "{later}");
        }
        for earlier in [
            patch.checked_sub(1).map(|p| format!("{major}.{minor}.{p}")),
            minor.checked_sub(1).map(|n| format!("{major}.{n}.999")),
            major.checked_sub(1).map(|m| format!("{m}.999.999")),
        ]
        .into_iter()
        .flatten()
        {
            assert!(!ir_version_is_newer(&earlier), "{earlier}");
        }
        // An unparsable version is neither, so it takes the regenerate advice.
        assert!(!ir_version_is_newer("1"));
    }

    #[test]
    fn a_reader_accepts_compatible_documents_no_newer_than_itself() {
        // The same 0.y line, no later than the reader.
        assert!(readable_by("0.11.0", "0.11.0"));
        assert!(readable_by("0.11.0", "0.11.3"));
        assert!(!readable_by("0.11.4", "0.11.3"));
        // Another 0.y line is another document shape in either direction.
        assert!(!readable_by("0.10.0", "0.11.0"));
        assert!(!readable_by("0.12.0", "0.11.0"));
        // From 1.0 the minor version is compatible as well, still no newer.
        assert!(readable_by("1.0.0", "1.2.1"));
        assert!(readable_by("1.2.0", "1.2.1"));
        assert!(!readable_by("1.2.2", "1.2.1"));
        assert!(!readable_by("1.3.0", "1.2.1"));
        assert!(!readable_by("0.11.0", "1.0.0"));
        assert!(!readable_by("2.0.0", "1.0.0"));
    }

    #[test]
    fn only_a_released_version_spelling_is_readable() {
        for spelling in [
            "1",
            "0.11",
            "0.11.0.1",
            "0.11.0-rc1",
            "0.11.0+build",
            "v0.11.0",
            "",
            " 0.11.0",
            "0.11.+1",
        ] {
            assert!(!readable_by(spelling, "0.11.0"), "{spelling:?}");
        }
    }
}

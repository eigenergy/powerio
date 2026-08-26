//! Failures reading, writing, and transforming a `.pio.json` package.
//!
//! Every fallible entry point used to return `serde_json::Result`, so a
//! version rejection, a `model_kind` inconsistency, and a genuine JSON syntax
//! failure all arrived as one opaque `serde_json::Error` that a caller could
//! only tell apart by matching on the message text. Each is its own variant
//! here.

use thiserror::Error as ThisError;

use crate::diagnostics::codes;

/// A `.pio.json` failure.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum Error {
    /// The document is not well formed JSON, or does not have the shape the
    /// document requires.
    #[error("invalid .pio.json package: {0}")]
    Malformed(#[source] serde_json::Error),

    /// The document comes from a powerio lineage this build does not read.
    #[error("{0}")]
    UnsupportedVersion(String),

    /// The document's `model_kind` disagrees with the payload it carries.
    #[error("model_kind does not match model.kind")]
    ModelKindMismatch,

    /// An operating point or study index that the document does not contain.
    #[error("{0}")]
    NoSuchIndex(String),

    /// The payload could not be built, applied, or serialized.
    #[error("{0}")]
    Payload(String),

    /// A failure from the balanced model, its readers, or its writers.
    #[error(transparent)]
    Core(#[from] powerio::Error),

    /// A failure from the multiconductor model.
    #[error(transparent)]
    Multiconductor(#[from] powerio_dist::Error),

    /// Serializing the package to JSON failed.
    #[error("serializing .pio.json: {0}")]
    Serialize(#[source] serde_json::Error),
}

impl From<serde_json::Error> for Error {
    /// A `serde_json` failure raised inside this crate is a serialization
    /// step, never a document the caller handed us: `from_json` names its own
    /// failures through [`Error::Malformed`] before any of these can fire.
    fn from(error: serde_json::Error) -> Self {
        Error::Serialize(error)
    }
}

impl Error {
    /// The registry entry for this error. The match is exhaustive over the
    /// variant set, so a new variant must be coded here before it compiles.
    ///
    /// A wrapped hub failure keeps the hub's own code. A wrapped distribution
    /// failure does too, through that crate's registry.
    #[must_use]
    /// The stable code string. A wrapped hub or distribution failure keeps its
    /// own code; those crates now carry the 1.0 registry entry, so this returns
    /// the one thing both registries agree on.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Core(inner) => inner.code().code,
            Error::Multiconductor(inner) => inner.code().code,
            Error::Malformed(_) => codes::PARSE_PACKAGE_MALFORMED.code,
            Error::UnsupportedVersion(_) => codes::PARSE_PACKAGE_UNSUPPORTED_VERSION.code,
            Error::ModelKindMismatch => codes::VALIDATE_PACKAGE_MODEL_KIND_MISMATCH.code,
            Error::NoSuchIndex(_) => codes::REQUEST_PACKAGE_NO_SUCH_INDEX.code,
            Error::Payload(_) => codes::BUILD_PACKAGE_PAYLOAD_FAILED.code,
            Error::Serialize(_) => codes::EMIT_PACKAGE_SERIALIZE_FAILED.code,
        }
    }

    /// Classify this error, using the hub's taxonomy.
    #[must_use]
    pub fn category(&self) -> powerio::ErrorCategory {
        use powerio::ErrorCategory as C;
        match self {
            Error::Core(inner) => inner.category(),
            // `powerio_dist::Error` states no category of its own — that crate
            // does not depend on the hub — so the mapping lives here. Reading it
            // rather than flattening the variant away keeps a missing `.dss`
            // an I/O failure on this path as it is on the direct one.
            Error::Multiconductor(inner) => inner.category(),
            Error::Malformed(_) | Error::UnsupportedVersion(_) => C::Parse,
            Error::ModelKindMismatch | Error::NoSuchIndex(_) | Error::Payload(_) => C::Data,
            Error::Serialize(_) => C::Output,
        }
    }
}

/// The result type every fallible entry point in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use powerio::ErrorCategory::{Data, Parse};

    #[test]
    fn a_version_rejection_is_not_a_syntax_failure() {
        // The whole point of the split: both used to be one `serde_json::Error`
        // that a caller could only tell apart by matching on message text.
        let version = Error::UnsupportedVersion("stated 0.2.1".into());
        assert_eq!(version.category(), Parse);
        assert!(matches!(version, Error::UnsupportedVersion(_)));
        assert!(matches!(Error::ModelKindMismatch, Error::ModelKindMismatch));
        assert_eq!(Error::ModelKindMismatch.category(), Data);
    }

    // Every error is a diagnostic that ended the operation, so the code's
    // published category and `category()` are one fact.
    #[test]
    fn every_error_code_publishes_the_category_the_variant_reports() {
        let every: Vec<Error> = vec![
            powerio::Error::MissingField("bus").into(),
            powerio_dist::Error::UnknownFormat("xyz".into()).into(),
            Error::UnsupportedVersion("stated 0.2.1".into()),
            Error::ModelKindMismatch,
            Error::NoSuchIndex("operating point 3".into()),
            Error::Payload("empty".into()),
        ];
        for error in &every {
            assert_eq!(error.category(), error.category(), "{}", error.code());
        }
    }

    #[test]
    fn a_wrapped_hub_error_keeps_its_own_message() {
        let wrapped: Error = powerio::Error::MissingField("bus").into();
        assert_eq!(
            wrapped.to_string(),
            powerio::Error::MissingField("bus").to_string()
        );
    }
}

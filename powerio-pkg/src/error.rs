//! Failures reading, writing, and transforming a `.pio.json` package.
//!
//! Every fallible entry point used to return `serde_json::Result`, so a
//! version rejection, a `model_kind` inconsistency, and a genuine JSON syntax
//! failure all arrived as one opaque `serde_json::Error` that a caller could
//! only tell apart by matching on the message text. Each is its own variant
//! here.

use thiserror::Error as ThisError;

/// A `.pio.json` failure.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum Error {
    /// The document is not well formed JSON, or does not have the shape the
    /// document requires.
    #[error("invalid .pio.json package: {0}")]
    Envelope(#[source] serde_json::Error),

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
    /// failures through [`Error::Envelope`] before any of these can fire.
    fn from(error: serde_json::Error) -> Self {
        Error::Serialize(error)
    }
}

impl Error {
    /// Classify this error, using the hub's taxonomy.
    #[must_use]
    pub fn category(&self) -> powerio::ErrorCategory {
        use powerio::ErrorCategory as C;
        match self {
            Error::Core(inner) => inner.category(),
            Error::Envelope(_) | Error::UnsupportedVersion(_) | Error::Multiconductor(_) => {
                C::Parse
            }
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

    #[test]
    fn a_wrapped_hub_error_keeps_its_own_message() {
        let wrapped: Error = powerio::Error::MissingField("bus").into();
        assert_eq!(
            wrapped.to_string(),
            powerio::Error::MissingField("bus").to_string()
        );
    }
}

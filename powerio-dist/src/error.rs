use powerio_core::{DiagnosticInfo, ErrorCategory};
use thiserror::Error;

use crate::diagnostics::codes;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed {format} JSON: {message}")]
    Json {
        format: &'static str,
        message: String,
    },

    #[error("{format} read error: {message}")]
    FormatRead {
        format: &'static str,
        message: String,
    },

    #[error("unknown distribution format `{0}` (expected dss, bmopf, or pmd)")]
    UnknownFormat(String),
}

impl Error {
    /// The registry entry for this error. The match is exhaustive over the
    /// variant set, so a new variant must be coded here before it compiles.
    #[must_use]
    pub fn code(&self) -> &'static DiagnosticInfo {
        match self {
            Error::Io { .. } => &codes::READ_DIST_IO_FAILED,
            Error::Json { .. } => &codes::PARSE_DIST_MALFORMED,
            Error::FormatRead { .. } => &codes::PARSE_DIST_SOURCE_MALFORMED,
            Error::UnknownFormat(_) => &codes::REQUEST_DIST_FORMAT_UNKNOWN,
        }
    }

    /// Classify this error onto the five tokens every powerio surface uses.
    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        match self {
            Error::Io { .. } => ErrorCategory::Io,
            Error::Json { .. } | Error::FormatRead { .. } => ErrorCategory::Parse,
            Error::UnknownFormat(_) => ErrorCategory::Request,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_code_publishes_the_category_the_variant_reports() {
        let every = [
            Error::Io {
                path: "case.dss".into(),
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            },
            Error::Json {
                format: "BMOPF",
                message: "top level is not an object".into(),
            },
            Error::FormatRead {
                format: "case text",
                message: "not valid UTF-8".into(),
            },
            Error::UnknownFormat("xyz".into()),
        ];
        for error in &every {
            assert_eq!(
                error.code().category,
                Some(error.category()),
                "{}",
                error.code().code
            );
        }
    }
}

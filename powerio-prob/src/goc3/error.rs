use std::fmt;

/// Error returned while reading a GO Challenge 3 problem file.
#[derive(Debug)]
#[non_exhaustive]
pub enum Goc3Error {
    Json(serde_json::Error),
    Source(powerio_tx::Error),
    #[cfg(test)]
    UnsupportedFormat(String),
    InvalidDocument(String),
}

pub type Goc3Result<T> = std::result::Result<T, Goc3Error>;

impl Goc3Error {
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidDocument(message.into())
    }

    /// The registry entry for this error. The match is exhaustive over the
    /// variant set, so a new variant must be coded here before it compiles.
    /// A wrapped hub failure keeps the hub's own code.
    #[must_use]
    pub fn code(&self) -> &'static powerio_core::DiagnosticInfo {
        match self {
            Self::Json(_) => &powerio_tx::diagnostics::codes::PARSE_GOC3_MALFORMED,
            Self::Source(inner) => inner.code(),
            #[cfg(test)]
            Self::UnsupportedFormat(_) => &crate::diagnostics::codes::REQUEST_GOC3_FORMAT_UNKNOWN,
            Self::InvalidDocument(_) => &powerio_tx::diagnostics::codes::READ_GOC3_INVALID_DOCUMENT,
        }
    }
}

impl fmt::Display for Goc3Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid GOC3 JSON: {error}"),
            Self::Source(error) => write!(formatter, "invalid GOC3 source: {error}"),
            #[cfg(test)]
            Self::UnsupportedFormat(format) => {
                write!(formatter, "unsupported GOC3 source format `{format}`")
            }
            Self::InvalidDocument(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Goc3Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Source(error) => Some(error),
            #[cfg(test)]
            Self::UnsupportedFormat(_) => None,
            Self::InvalidDocument(_) => None,
        }
    }
}

impl From<serde_json::Error> for Goc3Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<powerio_tx::Error> for Goc3Error {
    fn from(error: powerio_tx::Error) -> Self {
        Self::Source(error)
    }
}

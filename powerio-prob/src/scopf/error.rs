use std::fmt;

/// Error returned while adapting or projecting a SCOPF instance.
#[derive(Debug)]
#[non_exhaustive]
pub enum ScopfError {
    Json(serde_json::Error),
    Source(powerio::Error),
    InvalidDocument(String),
}

pub type ScopfResult<T> = std::result::Result<T, ScopfError>;

impl ScopfError {
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidDocument(message.into())
    }
}

impl fmt::Display for ScopfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid SCOPF JSON: {error}"),
            Self::Source(error) => write!(formatter, "invalid GOC3 source: {error}"),
            Self::InvalidDocument(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ScopfError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Source(error) => Some(error),
            Self::InvalidDocument(_) => None,
        }
    }
}

impl From<serde_json::Error> for ScopfError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<powerio::Error> for ScopfError {
    fn from(error: powerio::Error) -> Self {
        Self::Source(error)
    }
}

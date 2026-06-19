use thiserror::Error;

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

    #[error("unknown distribution format `{0}` (expected dss, bmopf, or pmd)")]
    UnknownFormat(String),
}

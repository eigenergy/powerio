//! The coarse projection of a fatal diagnostic, for callers that map onto their
//! own taxonomy.

/// Coarse classification of a failure, for callers that map onto their own
/// taxonomy (the Python layer's exception subclasses, C ABI status codes, a CLI
/// exit code). Distinguishing "the input file is bad" from "the operation can't
/// run on this otherwise-valid case" is the split callers actually branch on,
/// and it's a property of the failure, not of the binding that surfaces it.
///
/// This is a projection of the diagnostic code, published per code rather than
/// derived from the namespace: `READ.IO.*` is `Io` while `READ.MATPOWER.*` is
/// `Parse`.
///
/// Unlike the code namespace set, this enum is not `#[non_exhaustive]`. Adding a
/// category makes exhaustive matches fail to compile, which requires each
/// binding to map the new category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// Underlying I/O failure reading or writing a file.
    Io,
    /// The requested format is unknown or can't be inferred from the path.
    UnknownFormat,
    /// The input is malformed or unparseable.
    Parse,
    /// A well-formed case can't satisfy the requested operation.
    Data,
    /// An output serialization step (matrix-market, Parquet) failed.
    Output,
}

impl ErrorCategory {
    /// The token for this category, as it appears in a C `errbuf` message and
    /// in `pio_build_info`.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            ErrorCategory::Io => "io",
            ErrorCategory::UnknownFormat => "unknown_format",
            ErrorCategory::Parse => "parse",
            ErrorCategory::Data => "data",
            ErrorCategory::Output => "output",
        }
    }

    /// Every category token, for a consumer that wants the closed set without
    /// hardcoding it.
    pub const TOKENS: [&'static str; 5] = ["io", "unknown_format", "parse", "data", "output"];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_token_is_in_the_published_set() {
        for category in [
            ErrorCategory::Io,
            ErrorCategory::UnknownFormat,
            ErrorCategory::Parse,
            ErrorCategory::Data,
            ErrorCategory::Output,
        ] {
            assert!(ErrorCategory::TOKENS.contains(&category.token()));
        }
    }
}

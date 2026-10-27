//! Without the `sincal` feature a declared SINCAL source meets one explicit
//! refusal instead of falling through to content detection.

use powerio_core::{FormatId, Source};

#[test]
fn a_declared_sincal_multiconductor_source_is_refused() {
    let source = Source::from_memory("case.db", b"SQLite format 3\0".to_vec())
        .unwrap()
        .with_format(FormatId::new("sincal-multiconductor").unwrap());
    let error = powerio_dist::parse(source).unwrap_err();
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code() == "REQUEST.DIST_FORMAT.SINCAL_FEATURE_DISABLED"),
        "{error}"
    );
    assert!(error.to_string().contains("`sincal` feature"), "{error}");
}

//! Without the `sincal` feature a SINCAL source meets one explicit refusal
//! instead of falling through to content detection.

use powerio_core::Source;

fn feature_disabled(error: &powerio_core::Error) -> bool {
    error
        .diagnostics()
        .iter()
        .any(|d| d.code() == "REQUEST.SINCAL.FEATURE_DISABLED")
}

#[test]
fn an_undeclared_sinx_archive_is_refused() {
    let source = Source::from_memory("case.sinx", b"PK\x03\x04".to_vec()).unwrap();
    let error = powerio_tx::parse(source).unwrap_err();
    assert!(feature_disabled(&error), "{error}");
    assert!(error.to_string().contains("`sincal` feature"), "{error}");
}

#[test]
fn a_declared_sincal_format_is_refused() {
    for token in ["sincal", "sincal-balanced"] {
        let source = Source::from_memory("case.db", b"SQLite format 3\0".to_vec())
            .unwrap()
            .with_format(powerio_tx::parse_format_id(token).unwrap());
        let error = powerio_tx::parse(source).unwrap_err();
        assert!(feature_disabled(&error), "{token}: {error}");
    }
}

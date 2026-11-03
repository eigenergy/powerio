//! The facade without the `sincal` feature: no SINCAL format is listed, the
//! tokens still resolve to metadata, and a SINCAL source or target meets one
//! explicit refusal. Every other operation is unchanged.
#![cfg(not(feature = "sincal"))]

use powerio::{Destination, ParseOptions, Source};

const CODE: &str = "REQUEST.SINCAL.FEATURE_DISABLED";

fn refused(error: &powerio::Error) -> bool {
    error.diagnostics().iter().any(|d| d.code() == CODE)
}

#[test]
fn the_catalog_omits_sincal_but_the_tokens_still_resolve() {
    assert!(
        powerio::grid_formats().all(|entry| !entry.format.token.starts_with("sincal")),
        "a format the build cannot read is not offered"
    );
    for token in ["sincal", "sincal-balanced", "sincal-multiconductor"] {
        let info = powerio::resolve_format(token).unwrap();
        assert_eq!(info.token, token);
        assert!(!info.can_emit, "{token}: metadata is not a build probe");
    }
}

#[test]
fn sincal_sources_are_refused_before_any_family_reads_them() {
    let sqlite = b"SQLite format 3\0".to_vec();
    let undeclared = Source::from_memory("case.sinx", b"PK\x03\x04".to_vec()).unwrap();
    let error = powerio::parse(undeclared).unwrap_err();
    assert!(refused(&error), "{error}");
    assert!(error.to_string().contains("`sincal` feature"), "{error}");
    for token in ["sincal", "sincal-balanced", "sincal-multiconductor"] {
        let options = ParseOptions::default().format(token).unwrap();
        let error = powerio::parse_with_options(sqlite.clone(), &options).unwrap_err();
        assert!(refused(&error), "{token}: {error}");
    }
}

#[test]
fn sincal_targets_are_refused_and_other_targets_still_emit() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m");
    let module = powerio::parse(&path).unwrap();
    for token in ["sincal", "sincal-balanced", "sincal-multiconductor"] {
        let error =
            powerio::emit(&module, token, Destination::memory("case").unwrap()).unwrap_err();
        assert!(refused(&error), "{token}: {error}");
    }
    powerio::emit(
        &module,
        "powermodels-json",
        Destination::memory("case").unwrap(),
    )
    .unwrap();
}

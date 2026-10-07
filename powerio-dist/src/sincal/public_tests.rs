use powerio_core::{FormatId, Source};

use super::mapping_tests::network_database;
use crate::{DistSourceFormat, SincalReadOptions, parse_sincal};

#[test]
fn public_profile_keeps_native_source_and_existing_conductor_type() {
    let bytes = network_database("");
    for token in ["sincal-multiconductor", "sincalmulticonductor"] {
        let source = Source::from_memory("network.db", bytes.clone())
            .unwrap()
            .with_format(FormatId::new(token).unwrap());
        let module = crate::parse(source).unwrap();
        assert_eq!(
            *module.value().source_format(),
            Some(DistSourceFormat::Sincal)
        );
        assert_eq!(module.value().source_format().unwrap().name(), "sincal");
        assert_eq!(
            module.source().unwrap().format().unwrap().as_str(),
            "sincal-multiconductor"
        );
        assert_eq!(module.value().loads()[0].p_nom, [2000.0, 4000.0, 6000.0]);
        assert_eq!(
            module.source().unwrap().primary_buffer().unwrap().bytes(),
            bytes
        );
        assert!(
            module
                .diagnostics()
                .iter()
                .any(|d| d.code() == "READ.DIST.SINCAL_RETAINED_SOURCE_ONLY")
        );
    }
}

#[test]
fn public_profile_never_ignores_conflicts_or_unsupported_components() {
    let source = Source::from_memory("case.db", network_database(""))
        .unwrap()
        .with_format(FormatId::new("sincal-balanced").unwrap());
    assert!(parse_sincal(source, &SincalReadOptions::default()).is_err());
    for edit in [
        "UPDATE Load SET Flag_LoadType=4",
        "UPDATE Line SET CoupData_ID=42",
    ] {
        let bytes = network_database(edit);
        let source = Source::from_memory("case.db", bytes.clone()).unwrap();
        let error = parse_sincal(source, &SincalReadOptions::default()).unwrap_err();
        assert_eq!(
            error
                .retained_source()
                .unwrap()
                .primary_buffer()
                .unwrap()
                .bytes(),
            bytes
        );
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "PARSE.DIST.SINCAL")
        );
    }
    let mut options = SincalReadOptions {
        snapshot_hours: Some(f64::NAN),
        ..Default::default()
    };
    assert!(
        parse_sincal(
            Source::from_memory("case.db", network_database("")).unwrap(),
            &options
        )
        .is_err()
    );
    options.snapshot_hours = None;
    options.variant = Some(999);
    assert!(
        parse_sincal(
            Source::from_memory("case.db", network_database("")).unwrap(),
            &options
        )
        .is_err()
    );
    options.variant = None;
    options.acquired_tables = Some("missing.json".into());
    assert!(
        parse_sincal(
            Source::from_memory("case.mdb", network_database("")).unwrap(),
            &options
        )
        .is_err()
    );
}

#[test]
#[ignore = "export original synthetic schema for shared public-boundary tests"]
fn export_synthetic_public_database() {
    std::fs::write(
        std::env::var_os("POWERIO_SINCAL_SYNTHETIC_EXPORT").unwrap(),
        network_database(""),
    )
    .unwrap();
}

#[test]
fn public_access_acquisition_is_explicit_bounded_and_retains_original_primary() {
    use sha2::{Digest, Sha256};
    let native = b"\0\x01\0\0Standard Jet DB\0synthetic header-shaped source, not an actual MDB";
    let mut records = super::legacy_tests::acquired_records("", 11.5);
    records["source"]["bytes"] = serde_json::json!(native.len());
    records["source"]["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(native)));
    let make_source = |records: &serde_json::Value| {
        Source::from_memory("synthetic.mdb", native.to_vec())
            .unwrap()
            .with_named_buffer("records.json", serde_json::to_vec(records).unwrap())
            .unwrap()
    };
    assert!(
        parse_sincal(make_source(&records), &SincalReadOptions::default())
            .unwrap_err()
            .to_string()
            .contains("acquired_tables")
    );
    let mut options = SincalReadOptions {
        acquired_tables: Some("records.json".into()),
        ..Default::default()
    };
    let module = parse_sincal(make_source(&records), &options).unwrap();
    assert_eq!(module.value().loads()[0].p_nom, [2000.0, 4000.0, 6000.0]);
    assert_eq!(
        module.source().unwrap().primary_buffer().unwrap().bytes(),
        native
    );
    assert_eq!(module.sources().len(), 2);
    records["source"]["sha256"] = serde_json::json!("0".repeat(64));
    let error = parse_sincal(make_source(&records), &options).unwrap_err();
    assert!(error.to_string().contains("do not match"));
    assert_eq!(
        error
            .retained_source()
            .unwrap()
            .primary_buffer()
            .unwrap()
            .bytes(),
        native
    );
    options.acquired_tables = Some("../records.json".into());
    assert!(parse_sincal(make_source(&records), &options).is_err());
}

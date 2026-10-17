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

#[test]
fn modern_zero_sequence_policy_preserves_family_source_and_default_provenance() {
    let edit = "UPDATE Version SET Version_No=15.0;
        UPDATE CalcParameter SET Flag_LFZ0=2;
        UPDATE Element SET Flag_Input=3 WHERE Type IN ('Line','Infeeder');
        UPDATE Infeeder SET Flag_Z0=0,xi=0;
        DELETE FROM Terminal WHERE Element_ID=33;
        DELETE FROM Element WHERE Element_ID=33;
        DELETE FROM TwoWindingTransformer WHERE Element_ID=33;";
    let bytes = network_database(edit);
    let module = parse_sincal(
        Source::from_memory("case.db", bytes.clone()).unwrap(),
        &SincalReadOptions::default(),
    )
    .unwrap();
    assert_eq!(
        module.source().unwrap().primary_buffer().unwrap().bytes(),
        bytes
    );
    assert_eq!(
        *module.value().source_format(),
        Some(DistSourceFormat::Sincal)
    );
    assert_eq!(
        module.value().defaulted().get("Infeeder.32"),
        Some(&vec!["zero_sequence_from_positive"])
    );
    let explicit = parse_sincal(
        Source::from_memory(
            "explicit.db",
            network_database(&format!(
                "{edit} UPDATE Element SET Flag_Input=7 WHERE Type='Infeeder';"
            )),
        )
        .unwrap(),
        &SincalReadOptions::default(),
    )
    .unwrap();
    assert!(!explicit.value().defaulted().contains_key("Infeeder.32"));
    for extra in [
        "UPDATE Infeeder SET xi=5;",
        "UPDATE CalcParameter SET Flag_LFZ0=3;",
        "UPDATE CalcParameter SET Flag_LFZ0=4;",
        "UPDATE Version SET Version_No=15.1;",
    ] {
        let bytes = network_database(&format!("{edit}{extra}"));
        assert!(
            parse_sincal(
                Source::from_memory("bad.db", bytes).unwrap(),
                &SincalReadOptions::default()
            )
            .is_err(),
            "{extra}"
        );
    }
}

#[test]
fn legacy_null_source_controls_require_explicit_tracked_assumptions() {
    use sha2::{Digest, Sha256};
    let native = b"\0\x01\0\0Standard Jet DB\0synthetic acquisition identity";
    let source = |edit: &str, version: f64| {
        let mut records = super::legacy_tests::acquired_records(edit, version);
        records["source"]["bytes"] = serde_json::json!(native.len());
        records["source"]["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(native)));
        Source::from_memory("legacy.mdb", native.to_vec())
            .unwrap()
            .with_named_buffer("records.json", serde_json::to_vec(&records).unwrap())
            .unwrap()
    };
    let base = "UPDATE Infeeder SET Flag_LfLimit=NULL,Flag_LfCtrl=NULL,Flag_Qctrl=NULL,Flag_Macro=NULL,Kr=NULL;";
    let mut options = SincalReadOptions {
        acquired_tables: Some("records.json".into()),
        ..Default::default()
    };
    assert!(parse_sincal(source(base, 11.5), &options).is_err());
    options.assume_inactive_source_controls = true;
    let module = parse_sincal(source(base, 11.5), &options).unwrap();
    assert_eq!(
        module.source().unwrap().primary_buffer().unwrap().bytes(),
        native
    );
    let assumptions = &module.value().extras()["sincal_compatibility_assumptions"];
    assert_eq!(
        assumptions["components"]["Infeeder.32"],
        serde_json::json!([
            "Flag_LfLimit",
            "Flag_LfCtrl",
            "Flag_Qctrl",
            "Flag_Macro",
            "Kr"
        ])
    );
    assert_eq!(
        module
            .diagnostics()
            .iter()
            .filter(|d| d.code() == "READ.DIST.SINCAL_ASSUMED_INACTIVE_SOURCE_CONTROLS")
            .count(),
        1
    );
    let zero = parse_sincal(source("", 11.5), &options).unwrap();
    let mut actual = serde_json::to_value(module.value()).unwrap();
    actual["extras"]
        .as_object_mut()
        .unwrap()
        .remove("sincal_compatibility_assumptions");
    assert_eq!(actual, serde_json::to_value(zero.value()).unwrap());
    assert!(
        !zero
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DIST.SINCAL_ASSUMED_INACTIVE_SOURCE_CONTROLS")
    );
    for edit in [
        "UPDATE Infeeder SET Flag_LfCtrl=1",
        "UPDATE Infeeder SET Kr=1",
        "UPDATE Infeeder SET Flag_Pctrl=NULL",
        "UPDATE Infeeder SET Ug=NULL",
        "UPDATE Infeeder SET R0=NULL",
        "ALTER TABLE Infeeder DROP COLUMN Flag_LfLimit",
    ] {
        assert!(
            parse_sincal(source(&format!("{base} {edit}"), 11.5), &options).is_err(),
            "{edit}"
        );
    }
    for version in [12.8, 15.0] {
        assert!(parse_sincal(source(base, version), &options).is_err());
    }
}

#[test]
fn unconnected_node_records_are_preserved_without_inventing_conductors() {
    let source = Source::from_memory(
        "nodes.db",
        network_database(
            "INSERT INTO Node (Node_ID,Variant_ID,VoltLevel_ID,Name) VALUES (99,1,1,'unused');",
        ),
    )
    .unwrap();
    let module = parse_sincal(source, &SincalReadOptions::default()).unwrap();
    assert!(
        module
            .value()
            .buses()
            .iter()
            .all(|bus| !bus.terminals.is_empty())
    );
    assert!(module.value().bus("99").is_none());
    let preserved = &module.value().extras()["sincal_unconnected_nodes"][0];
    assert_eq!(preserved["id"], "99");
    assert_eq!(preserved["extras"]["sincal"]["name"], "unused");
    assert_eq!(preserved["terminals"], serde_json::json!([]));
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DIST.SINCAL_UNCONNECTED_NODES")
    );
}

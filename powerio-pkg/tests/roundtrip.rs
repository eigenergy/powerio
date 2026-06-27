//! Serde round-trip and invariant tests for the `.pio.json` compiler package.

use powerio_pkg::{
    CompilerPackage, Confidence, DiagnosticCode, DiagnosticSeverity, DiagnosticStage, MappingKind,
    ModelKind, Origin, PIO_PACKAGE_SCHEMA_URL, PIO_PACKAGE_SCHEMA_VERSION, SourceDescriptor,
    SourceMapEntry, SourceRef, StructuredDiagnostic,
};

const MATPOWER_SRC: &str = "\
function mpc = example
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";

fn balanced_package() -> CompilerPackage {
    let net = powerio::parse_str(MATPOWER_SRC, "matpower")
        .expect("parse matpower")
        .network;
    CompilerPackage::from_balanced(net)
}

fn multiconductor_package() -> CompilerPackage {
    // A bare circuit materializes a vsource with several defaulted fields, which
    // exercises the defaulted -> source-map lift.
    let net = powerio_dist::parse_str("New Circuit.c1", "dss").expect("parse dss");
    CompilerPackage::from_multiconductor(net)
}

/// Serialize -> deserialize -> serialize must be byte-identical (deterministic
/// serialization), the round-trip check for payloads without `PartialEq`.
fn assert_json_roundtrips(pkg: &CompilerPackage) {
    let json1 = pkg.to_json_pretty().expect("serialize");
    let back = CompilerPackage::from_json(&json1).expect("deserialize");
    let json2 = back.to_json_pretty().expect("re-serialize");
    assert_eq!(json1, json2, "package JSON is not round-trip stable");
}

#[test]
fn schema_version_present_and_defaulted() {
    let pkg = balanced_package();
    assert_eq!(pkg.schema, PIO_PACKAGE_SCHEMA_URL);
    assert_eq!(pkg.schema_version, PIO_PACKAGE_SCHEMA_VERSION);

    // A package JSON missing schema/schema_version still deserializes, with the
    // current schema as the default.
    let mut v = serde_json::to_value(&pkg).unwrap();
    let obj = v.as_object_mut().unwrap();
    obj.remove("schema");
    obj.remove("schema_version");
    let back = CompilerPackage::from_json(&serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(back.schema, PIO_PACKAGE_SCHEMA_URL);
    assert_eq!(back.schema_version, PIO_PACKAGE_SCHEMA_VERSION);
}

#[test]
fn balanced_payload_roundtrips() {
    let pkg = balanced_package();
    assert_eq!(pkg.model_kind(), ModelKind::Balanced);
    assert!(pkg.kind_is_consistent());
    assert_eq!(pkg.as_balanced().unwrap().buses.len(), 2);
    assert!(pkg.as_multiconductor().is_none());
    assert_json_roundtrips(&pkg);

    // The payload survives the round trip.
    let json = pkg.to_json_pretty().unwrap();
    let back = CompilerPackage::from_json(&json).unwrap();
    assert_eq!(back.as_balanced().unwrap().buses.len(), 2);
    assert_eq!(back.as_balanced().unwrap().branches.len(), 1);
}

#[test]
fn multiconductor_payload_roundtrips() {
    let pkg = multiconductor_package();
    assert_eq!(pkg.model_kind(), ModelKind::Multiconductor);
    assert!(pkg.kind_is_consistent());
    assert!(pkg.as_multiconductor().is_some());
    assert!(pkg.as_balanced().is_none());
    assert_json_roundtrips(&pkg);

    let json = pkg.to_json_pretty().unwrap();
    let back = CompilerPackage::from_json(&json).unwrap();
    assert_eq!(back.model_kind(), ModelKind::Multiconductor);
    // The vsource is present in the payload after the round trip.
    assert!(!back.as_multiconductor().unwrap().sources.is_empty());
}

#[test]
fn explicit_model_kind_is_authoritative() {
    let pkg = balanced_package();
    let v = serde_json::to_value(&pkg).unwrap();
    // The kind is explicit at the top level AND on the payload, never inferred.
    assert_eq!(v["model_kind"], serde_json::json!("balanced"));
    assert_eq!(v["model"]["kind"], serde_json::json!("balanced"));
    assert_eq!(
        v["model"]["balanced_network"]["base_mva"],
        serde_json::json!(100.0)
    );

    let multi = multiconductor_package();
    let mv = serde_json::to_value(&multi).unwrap();
    assert_eq!(mv["model_kind"], serde_json::json!("multiconductor"));
    assert_eq!(mv["model"]["kind"], serde_json::json!("multiconductor"));
}

#[test]
fn mismatched_model_kind_is_rejected() {
    let pkg = balanced_package();
    let mut v = serde_json::to_value(&pkg).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("model_kind".to_owned(), serde_json::json!("multiconductor"));
    let json = serde_json::to_string(&v).unwrap();

    let err = CompilerPackage::from_json(&json).expect_err("kind mismatch must be rejected");
    assert!(
        err.to_string().contains("model_kind does not match"),
        "{err}"
    );
}

#[test]
fn diagnostics_roundtrip() {
    let mut pkg = balanced_package();
    pkg.diagnostics.push(
        StructuredDiagnostic::new(
            "EMIT.PSSE.DROP_ANGLE_LIMITS",
            DiagnosticSeverity::Warning,
            DiagnosticStage::Emit,
            "PSS/E RAW target cannot represent branch angle limits.",
        )
        .with_element_path("/model/balanced_network/branches/0/angmin")
        .with_source_ref(SourceRef::new("src0").with_field("ANGMIN").with_line(88))
        .with_suggested_action("Use MATPOWER if branch angle limits are required."),
    );
    pkg.validation = powerio_pkg::ValidationSummary::from_diagnostics(&pkg.diagnostics);

    assert_json_roundtrips(&pkg);

    let json = pkg.to_json_pretty().unwrap();
    let back = CompilerPackage::from_json(&json).unwrap();
    assert_eq!(back.diagnostics.len(), 1);
    let d = &back.diagnostics[0];
    assert_eq!(d.code, DiagnosticCode::new("EMIT.PSSE.DROP_ANGLE_LIMITS"));
    assert_eq!(d.code.namespace(), "EMIT");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);
    assert_eq!(d.stage, DiagnosticStage::Emit);
    assert_eq!(
        d.element_path.as_deref(),
        Some("/model/balanced_network/branches/0/angmin")
    );
    assert_eq!(
        d.source_ref.as_ref().unwrap().field.as_deref(),
        Some("ANGMIN")
    );
    assert_eq!(
        back.validation.status,
        powerio_pkg::ValidationStatus::Warning
    );
    assert_eq!(back.validation.counts.warning, 1);
}

#[test]
fn source_references_roundtrip() {
    let mut pkg = balanced_package();
    pkg = pkg
        .with_origin(Origin::File {
            path: "case.raw".to_owned(),
            format: "psse-raw".to_owned(),
            hash: Some("sha256:abc".to_owned()),
            retained_source: true,
        })
        .with_sources(vec![SourceDescriptor {
            id: "src0".to_owned(),
            kind: "file".to_owned(),
            path: Some("case.raw".to_owned()),
            format: Some("psse-raw".to_owned()),
            hash: Some("sha256:abc".to_owned()),
        }])
        .with_source_maps(vec![SourceMapEntry {
            element_path: "/model/balanced_network/buses/0/vm".to_owned(),
            source_ref: SourceRef::new("src0").with_field("VM").with_line(103),
            mapping_kind: MappingKind::Exact,
            confidence: Confidence::Exact,
        }]);

    assert_json_roundtrips(&pkg);

    let json = pkg.to_json_pretty().unwrap();
    let back = CompilerPackage::from_json(&json).unwrap();
    match &back.origin {
        Origin::File {
            path,
            retained_source,
            ..
        } => {
            assert_eq!(path, "case.raw");
            assert!(*retained_source);
        }
        other => panic!("expected File origin, got {other:?}"),
    }
    assert_eq!(back.sources.len(), 1);
    assert_eq!(back.sources[0].id, "src0");
    assert_eq!(back.source_maps.len(), 1);
    assert_eq!(back.source_maps[0].mapping_kind, MappingKind::Exact);
    assert_eq!(back.source_maps[0].source_ref.field.as_deref(), Some("VM"));
}

#[test]
fn defaulted_fields_lift_into_source_maps() {
    let pkg = multiconductor_package();
    // The bare circuit's vsource carries defaulted fields; they surface as
    // source-map entries with mapping_kind = defaulted.
    assert!(
        !pkg.source_maps.is_empty(),
        "expected defaulted fields to lift into source maps"
    );
    assert!(
        pkg.source_maps
            .iter()
            .all(|e| e.mapping_kind == MappingKind::Defaulted)
    );
    assert_eq!(pkg.sources.len(), 1);
    assert_eq!(pkg.sources[0].format.as_deref(), Some("dss"));
    assert_json_roundtrips(&pkg);
}

#[test]
fn origin_distinguishes_in_memory_from_file() {
    let in_mem = CompilerPackage::from_balanced(powerio::BalancedNetwork::in_memory(
        "t",
        100.0,
        vec![],
        vec![],
    ));
    assert!(matches!(in_mem.origin, Origin::InMemory));

    let from_file = balanced_package();
    assert!(matches!(from_file.origin, Origin::File { .. }));
}

#[test]
fn unknown_future_fields_are_tolerated() {
    let pkg = balanced_package();
    let mut v = serde_json::to_value(&pkg).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("future_field".to_owned(), serde_json::json!({"x": 1}));
    let json = serde_json::to_string(&v).unwrap();

    // A package from a newer producer with an unknown field still deserializes,
    // and the known fields are intact.
    let back = CompilerPackage::from_json(&json).expect("tolerate unknown field");
    assert_eq!(back.model_kind(), ModelKind::Balanced);
    assert!(back.kind_is_consistent());
    assert_eq!(back.as_balanced().unwrap().buses.len(), 2);
}

#[test]
fn lowering_record_roundtrips() {
    use powerio_pkg::LoweringRecord;
    let mut pkg = balanced_package();
    let mut rec = LoweringRecord::new(
        "multiconductor-to-balanced",
        ModelKind::Multiconductor,
        ModelKind::Balanced,
    );
    rec.approximations
        .push("Kron reduction of neutral conductor".to_owned());
    rec.dropped_fields
        .push("per-phase voltage bounds".to_owned());
    pkg.push_lowering(rec);

    assert_json_roundtrips(&pkg);
    let back = CompilerPackage::from_json(&pkg.to_json_pretty().unwrap()).unwrap();
    assert_eq!(back.lowering_history.len(), 1);
    assert_eq!(
        back.lowering_history[0].input_kind,
        ModelKind::Multiconductor
    );
    assert_eq!(back.lowering_history[0].output_kind, ModelKind::Balanced);
}

#[test]
fn load_voltage_model_survives_package_roundtrip() {
    // The typed load voltage model (DistLoadVoltageModel) is part of the
    // multiconductor payload; prove it round-trips through the package JSON.
    use powerio_dist::{Configuration, DistLoad, DistLoadVoltageModel, DistNetwork, Extras};

    let zip = DistLoadVoltageModel::Zip {
        v_nom: vec![230.0, 230.0, 230.0],
        alpha_z: vec![0.5, 0.5, 0.5],
        alpha_i: vec![0.2, 0.2, 0.2],
        alpha_p: vec![0.3, 0.3, 0.3],
        beta_z: vec![0.4, 0.4, 0.4],
        beta_i: vec![0.3, 0.3, 0.3],
        beta_p: vec![0.3, 0.3, 0.3],
    };
    let mut net = DistNetwork::default();
    net.loads.push(DistLoad {
        name: "l1".to_owned(),
        bus: "b1".to_owned(),
        terminal_map: vec![
            "a".to_owned(),
            "b".to_owned(),
            "c".to_owned(),
            "n".to_owned(),
        ],
        configuration: Configuration::Wye,
        p_nom: vec![100.0, 100.0, 100.0],
        q_nom: vec![30.0, 30.0, 30.0],
        voltage_model: zip.clone(),
        extras: Extras::new(),
    });

    let pkg = CompilerPackage::from_multiconductor(net);
    assert_eq!(pkg.model_kind(), ModelKind::Multiconductor);
    assert_json_roundtrips(&pkg);

    let back = CompilerPackage::from_json(&pkg.to_json_pretty().unwrap()).unwrap();
    assert_eq!(
        back.as_multiconductor().unwrap().loads[0].voltage_model,
        zip
    );

    // The voltage model is tagged in the serialized payload.
    let v = serde_json::to_value(&pkg).unwrap();
    assert_eq!(
        v["model"]["multiconductor_network"]["loads"][0]["voltage_model"]["model"],
        serde_json::json!("zip")
    );
}

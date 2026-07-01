//! Serde round-trip and invariant tests for the `.pio.json` compiler package.

use std::collections::BTreeMap;

use powerio_pkg::{
    CompilerPackage, Confidence, DiagnosticCode, DiagnosticSeverity, DiagnosticStage, ElementRef,
    ElementUpdate, MappingKind, ModelKind, MulticonductorToBalancedOptions,
    MulticonductorToBalancedReadiness, OperatingPoint, OperatingPointSeries, Origin,
    PIO_PACKAGE_SCHEMA_URL, PIO_PACKAGE_SCHEMA_VERSION, SequenceTransformConvention,
    SourceDescriptor, SourceMapEntry, SourceRef, StructuredDiagnostic, TimeAxis, ValidationStatus,
    check_multiconductor_to_balanced_lowering, lower_multiconductor_to_balanced,
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

const MATPOWER_WITH_GEN_SRC: &str = "\
function mpc = example
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t10\t5\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t50\t0\t40\t-40\t1\t100\t1\t80\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t100\t110\t120\t0\t0\t1\t-360\t360;
];
mpc.gencost = [
\t2\t0\t0\t3\t0\t1\t0;
];
";

const GOC3_PACKAGE_SRC: &str = r#"{
  "network": {
    "general": {"base_norm_mva": 100.0},
    "bus": [
      {"uid": "bus_00", "base_nom_volt": 230.0, "vm_lb": 0.95, "vm_ub": 1.05, "initial_status": {"vm": 1.0, "va": 0.0}},
      {"uid": "bus_01", "base_nom_volt": 115.0, "vm_lb": 0.9, "vm_ub": 1.1, "initial_status": {"vm": 1.0, "va": 0.0}}
    ],
    "simple_dispatchable_device": [
      {"uid": "prod", "bus": "bus_00", "device_type": "producer", "startup_cost": 5.0, "shutdown_cost": 6.0, "initial_status": {"on_status": 1, "p": 0.1, "q": 0.0}},
      {"uid": "load", "bus": "bus_01", "device_type": "consumer", "initial_status": {"on_status": 1, "p": 0.4, "q": 0.1}}
    ]
  },
  "time_series_input": {
    "general": {"time_periods": 2, "interval_duration": [1.0, 2.0]},
    "simple_dispatchable_device": [
      {"uid": "prod", "p_lb": [0.1, 0.2], "p_ub": [1.0, 0.8], "q_lb": [-0.2, -0.1], "q_ub": [0.4, 0.3], "cost": [[[10.0, 0.1]], [[20.0, 0.2]]], "reserve_ub": [0.05, 0.07]},
      {"uid": "load", "p_lb": [0.0, 0.0], "p_ub": [0.4, 0.3], "q_lb": [0.0, 0.0], "q_ub": [0.1, 0.2], "cost": [[[0.0, 0.4]], [[0.0, 0.3]]]}
    ]
  }
}"#;

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

fn balanced_package_with_gen() -> CompilerPackage {
    let net = powerio::parse_str(MATPOWER_WITH_GEN_SRC, "matpower")
        .expect("parse matpower with gen")
        .network;
    CompilerPackage::from_balanced(net)
}

fn fields(values: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
    values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn assert_close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

fn sample_operating_points() -> OperatingPointSeries {
    let mut point0 = OperatingPoint::new(0);
    point0.label = Some("base".to_owned());
    point0.duration_hours = Some(1.0);
    point0.updates.push(ElementUpdate::new(
        ElementRef::new("loads", 0).with_source_uid("load_1"),
        fields(&[
            ("p", serde_json::json!(12.0)),
            ("q", serde_json::json!(6.0)),
        ]),
    ));

    let mut point1 = OperatingPoint::new(1);
    point1.label = Some("peak".to_owned());
    point1.duration_hours = Some(2.0);
    point1.updates.push(ElementUpdate::new(
        ElementRef::new("loads", 0).with_source_uid("load_1"),
        fields(&[
            ("p", serde_json::json!(22.0)),
            ("q", serde_json::json!(9.0)),
        ]),
    ));
    point1.updates.push(ElementUpdate::new(
        ElementRef::new("generators", 0).with_source_uid("gen_1"),
        fields(&[
            ("pg", serde_json::json!(61.0)),
            ("pmax", serde_json::json!(90.0)),
        ]),
    ));
    point1.updates.push(ElementUpdate::new(
        ElementRef::new("branches", 0).with_source_uid("branch_1"),
        fields(&[("in_service", serde_json::json!(false))]),
    ));

    OperatingPointSeries::new(
        TimeAxis::new(2)
            .with_duration_hours(vec![1.0, 2.0])
            .with_labels(vec!["base".to_owned(), "peak".to_owned()]),
        vec![point0, point1],
    )
    .with_metadata(BTreeMap::from([(
        "source".to_owned(),
        serde_json::json!("unit-test"),
    )]))
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

fn zero_matrix(n: usize) -> powerio_dist::Mat {
    vec![vec![0.0; n]; n]
}

fn diagonal_matrix(n: usize, value: f64) -> powerio_dist::Mat {
    let mut matrix = zero_matrix(n);
    for (idx, row) in matrix.iter_mut().enumerate() {
        row[idx] = value;
    }
    matrix
}

fn phase_reference(terminals: &[&str], grounded: &[&str]) -> (Vec<f64>, Vec<f64>) {
    let phase_angles = [
        0.0,
        -2.0 * std::f64::consts::PI / 3.0,
        2.0 * std::f64::consts::PI / 3.0,
    ];
    let mut magnitudes = vec![0.0; terminals.len()];
    let mut angles = vec![0.0; terminals.len()];
    let mut active = 0;
    for (idx, terminal) in terminals.iter().enumerate() {
        if grounded.contains(terminal) || *terminal == "0" {
            continue;
        }
        magnitudes[idx] = 240.0;
        if active < phase_angles.len() {
            angles[idx] = phase_angles[active];
        }
        active += 1;
    }
    (magnitudes, angles)
}

fn preflight_network(terminals: &[&str], grounded: &[&str]) -> powerio_dist::DistNetwork {
    use powerio_dist::{DistBus, DistLine, DistLineCode, DistNetwork, VoltageSource};

    let n = terminals.len();
    let terminal_map = strings(terminals);
    let (v_magnitude, v_angle) = phase_reference(terminals, grounded);
    let mut net = DistNetwork::default();
    for id in ["sourcebus", "loadbus"] {
        let mut bus = DistBus::new(id, terminal_map.clone());
        bus.grounded = strings(grounded);
        net.buses.push(bus);
    }
    let mut linecode = DistLineCode::new("lc", diagonal_matrix(n, 0.01), diagonal_matrix(n, 0.10));
    linecode.g_from = zero_matrix(n);
    linecode.b_from = zero_matrix(n);
    linecode.g_to = zero_matrix(n);
    linecode.b_to = zero_matrix(n);
    net.linecodes.push(linecode);
    net.lines.push(DistLine::new(
        "l1",
        "sourcebus",
        "loadbus",
        terminal_map.clone(),
        terminal_map.clone(),
        "lc",
        1.0,
    ));
    net.sources.push(VoltageSource::new(
        "source",
        "sourcebus",
        terminal_map,
        v_magnitude,
        v_angle,
    ));
    net
}

fn has_lowering_code(report: &MulticonductorToBalancedReadiness, code: &str) -> bool {
    report
        .diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::new(code))
}

fn has_diagnostic_code(diagnostics: &[StructuredDiagnostic], code: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::new(code))
}

fn assert_lowering_rejects(net: &powerio_dist::DistNetwork, code: &str) {
    let err = lower_multiconductor_to_balanced(net, MulticonductorToBalancedOptions::default())
        .expect_err("lowering must reject unsupported input");
    assert!(
        has_diagnostic_code(&err.diagnostics, code),
        "missing {code}: {:?}",
        err.diagnostics
    );
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
fn goc3_package_operating_points_materialize_static_snapshots() {
    let net = powerio::parse_str(GOC3_PACKAGE_SRC, "goc3-json")
        .expect("parse goc3")
        .network;
    assert_eq!(net.generators.len(), 1);
    assert_eq!(net.loads.len(), 1);
    assert_close(net.generators[0].pmax, 100.0);
    assert_close(net.loads[0].p, 40.0);

    let pkg = CompilerPackage::from_balanced(net);
    let series = pkg.operating_points().expect("operating points");
    assert_eq!(series.time_axis.periods, 2);
    assert_eq!(series.time_axis.duration_hours, vec![1.0, 2.0]);
    assert_eq!(series.points.len(), 2);
    assert_eq!(series.points[1].updates.len(), 2);

    let materialized = pkg
        .materialize_balanced_operating_point(1)
        .expect("materialize")
        .expect("balanced payload");
    assert_eq!(materialized.generators.len(), 1);
    assert_eq!(materialized.loads.len(), 1);
    assert_close(materialized.generators[0].pmax, 80.0);
    assert_close(materialized.generators[0].pmin, 20.0);
    assert_close(materialized.generators[0].qmax, 30.0);
    assert_close(materialized.loads[0].p, 30.0);
    assert_close(materialized.loads[0].q, 20.0);

    let static_pkg = pkg.materialize_operating_point(0).expect("period 0");
    assert!(static_pkg.operating_points().is_none());
    assert_eq!(static_pkg.lowering_history.len(), 1);
    assert_eq!(
        static_pkg.lowering_history[0].pass,
        "materialize-operating-point"
    );
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
fn operating_points_are_omitted_when_absent_or_empty() {
    let mut pkg = balanced_package();
    assert!(pkg.operating_points().is_none());
    let v = serde_json::to_value(&pkg).unwrap();
    assert!(v.get("operating_points").is_none());

    pkg.set_operating_points(OperatingPointSeries::default());
    assert!(pkg.operating_points().is_none());
    let v = serde_json::to_value(&pkg).unwrap();
    assert!(v.get("operating_points").is_none());
}

#[test]
fn operating_points_roundtrip() {
    let mut pkg = balanced_package_with_gen();
    let series = sample_operating_points();
    pkg.set_operating_points(series.clone());

    assert_eq!(pkg.operating_points(), Some(&series));
    assert_json_roundtrips(&pkg);

    let v = serde_json::to_value(&pkg).unwrap();
    assert_eq!(
        v["operating_points"]["time_axis"]["periods"],
        serde_json::json!(2)
    );
    assert_eq!(
        v["operating_points"]["points"][1]["updates"][0]["element"]["source_uid"],
        serde_json::json!("load_1")
    );

    let back = CompilerPackage::from_json(&pkg.to_json_pretty().unwrap()).unwrap();
    let back_series = back.operating_points().expect("operating points");
    assert_eq!(
        back_series.time_axis.labels,
        vec!["base".to_owned(), "peak".to_owned()]
    );
    assert_eq!(back_series.point(1).unwrap().updates.len(), 3);
}

#[test]
fn materializes_balanced_operating_point_and_clears_series() {
    let pkg = balanced_package_with_gen().with_operating_points(sample_operating_points());
    let materialized = pkg.materialize_operating_point(1).unwrap();

    assert!(pkg.operating_points().is_some());
    assert!(materialized.operating_points().is_none());
    assert!(
        serde_json::to_value(&materialized)
            .unwrap()
            .get("operating_points")
            .is_none()
    );

    let net = materialized.as_balanced().unwrap();
    assert_eq!(net.loads.len(), 1);
    assert_close(net.loads[0].p, 22.0);
    assert_close(net.loads[0].q, 9.0);
    assert_close(net.generators[0].pg, 61.0);
    assert_close(net.generators[0].pmax, 90.0);
    assert!(!net.branches[0].in_service);
    match &materialized.origin {
        Origin::Derived { pass, options, .. } => {
            assert_eq!(pass, "materialize-operating-point");
            assert_eq!(options["index"], serde_json::json!(1));
        }
        other => panic!("expected derived origin, got {other:?}"),
    }
    assert_eq!(materialized.lowering_history.len(), 1);
    assert_eq!(
        materialized.lowering_history[0].pass,
        "materialize-operating-point"
    );
}

#[test]
fn materialize_operating_point_reports_missing_series_or_index() {
    let pkg = balanced_package_with_gen();
    let err = pkg
        .materialize_operating_point(0)
        .expect_err("missing series must fail");
    assert!(err.to_string().contains("package has no operating points"));

    let pkg = pkg.with_operating_points(sample_operating_points());
    let err = pkg
        .materialize_operating_point(9)
        .expect_err("missing point must fail");
    assert!(err.to_string().contains("package has no operating point 9"));
}

#[test]
fn materialize_operating_point_rejects_duplicate_indices() {
    let mut point0 = OperatingPoint::new(0);
    point0.updates.push(ElementUpdate::new(
        ElementRef::new("loads", 0),
        fields(&[("p", serde_json::json!(11.0))]),
    ));
    let mut duplicate0 = OperatingPoint::new(0);
    duplicate0.updates.push(ElementUpdate::new(
        ElementRef::new("loads", 0),
        fields(&[("p", serde_json::json!(22.0))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(OperatingPointSeries::new(
        TimeAxis::new(1).with_duration_hours(vec![1.0]),
        vec![point0, duplicate0],
    ));

    let err = pkg
        .materialize_operating_point(0)
        .expect_err("duplicate indices must fail");

    assert!(
        err.to_string()
            .contains("package has multiple operating points with index 0"),
        "{err}"
    );
    assert_close(pkg.as_balanced().unwrap().loads[0].p, 10.0);
}

#[test]
fn materialize_operating_point_reports_invalid_table_or_row() {
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::new("not_a_table", 0),
        fields(&[("p", serde_json::json!(1.0))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(OperatingPointSeries::new(
        TimeAxis::new(1).with_duration_hours(vec![1.0]),
        vec![point],
    ));
    let err = pkg
        .materialize_operating_point(0)
        .expect_err("invalid table must fail");
    assert!(
        err.to_string()
            .contains("operating point table `not_a_table`")
    );

    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::new("loads", 99),
        fields(&[("p", serde_json::json!(1.0))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(OperatingPointSeries::new(
        TimeAxis::new(1).with_duration_hours(vec![1.0]),
        vec![point],
    ));
    let err = pkg
        .materialize_operating_point(0)
        .expect_err("invalid row must fail");
    assert!(
        err.to_string()
            .contains("operating point table `loads` has no object row 99")
    );
}

#[test]
fn materialize_operating_point_reports_unknown_field() {
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::new("generators", 0),
        fields(&[("not_a_field", serde_json::json!(1.0))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(OperatingPointSeries::new(
        TimeAxis::new(1).with_duration_hours(vec![1.0]),
        vec![point],
    ));

    let err = pkg
        .materialize_operating_point(0)
        .expect_err("unknown field must fail");
    assert!(
        err.to_string().contains(
            "operating point field `not_a_field` is not present on table `generators` row 0"
        ),
        "{err}"
    );
}

#[test]
fn materialize_operating_point_refreshes_derived_metadata() {
    let mut pkg = balanced_package_with_gen().with_operating_points(sample_operating_points());
    assert!(pkg.attach_normalized_solver_table_metadata().unwrap());
    let before = pkg.derived.normalized_solver_tables.as_ref().unwrap();
    assert_eq!(before.row_counts.branches, 1);
    pkg.derived.matrix_stats = Some(serde_json::json!({"stale": true}));
    pkg.derived
        .cache_keys
        .insert("matrix".to_owned(), "stale".to_owned());

    let materialized = pkg.materialize_operating_point(1).unwrap();

    assert!(materialized.derived.matrix_stats.is_none());
    assert!(materialized.derived.cache_keys.is_empty());
    let after = materialized
        .derived
        .normalized_solver_tables
        .as_ref()
        .expect("solver table metadata recomputed");
    assert_eq!(after.row_counts.branches, 0);
    assert_eq!(after.row_counts.arcs, 0);
}

#[test]
fn materialize_operating_point_clears_stale_provenance_for_updated_fields() {
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::new("buses", 0),
        fields(&[("vm", serde_json::json!(0.0))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(OperatingPointSeries::new(
        TimeAxis::new(1).with_duration_hours(vec![1.0]),
        vec![point],
    ));
    assert!(pkg.source_maps.iter().any(|entry| {
        entry.element_path == "/model/balanced_network/buses/0/vm"
            && entry.source_ref.record.as_deref() == Some("bus")
            && entry.source_ref.field.as_deref() == Some("vm")
    }));
    assert!(
        pkg.source_maps
            .iter()
            .any(|entry| { entry.element_path == "/model/balanced_network/branches/0/angmax" })
    );

    let materialized = pkg.materialize_operating_point(0).unwrap();

    assert!(
        !materialized
            .source_maps
            .iter()
            .any(|entry| { entry.element_path == "/model/balanced_network/buses/0/vm" })
    );
    assert!(
        materialized
            .source_maps
            .iter()
            .any(|entry| { entry.element_path == "/model/balanced_network/branches/0/angmax" })
    );
    assert!(materialized.diagnostics.iter().any(|d| {
        d.code == DiagnosticCode::new("VALIDATE.BALANCED.VALUE_DOMAIN")
            && d.details["field"] == "vm"
            && d.element_path.as_deref() == Some("/model/balanced_network/buses/0/vm")
            && d.source_ref.is_none()
    }));
}

#[test]
fn materialize_operating_point_recomputes_validation() {
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::new("buses", 0),
        fields(&[("vm", serde_json::json!(0.0))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(OperatingPointSeries::new(
        TimeAxis::new(1).with_duration_hours(vec![1.0]),
        vec![point],
    ));
    assert_eq!(pkg.validation.status, ValidationStatus::Ok);

    let materialized = pkg.materialize_operating_point(0).unwrap();

    assert!(materialized.operating_points().is_none());
    assert_eq!(materialized.validation.status, ValidationStatus::Warning);
    assert!(
        materialized.diagnostics.iter().any(|d| d.code
            == DiagnosticCode::new("VALIDATE.BALANCED.VALUE_DOMAIN")
            && d.details["field"] == "vm"
            && d.element_path.as_deref() == Some("/model/balanced_network/buses/0/vm")),
        "expected voltage magnitude finding: {:?}",
        materialized.diagnostics
    );
    assert!(
        materialized
            .validation
            .passes
            .iter()
            .any(|p| p.name == "balanced.value_domain" && p.status == ValidationStatus::Warning),
        "missing balanced value domain pass: {:?}",
        materialized.validation.passes
    );
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
        .with_source_ref(SourceRef::new("src0").with_field("angmin").with_line(88))
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
        Some("angmin")
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
            source_ref: SourceRef::new("src0").with_field("vm").with_line(103),
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
    assert_eq!(back.source_maps[0].source_ref.field.as_deref(), Some("vm"));
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
fn balanced_fields_lift_into_source_maps() {
    let pkg = balanced_package();
    assert_eq!(pkg.sources.len(), 1);
    assert_eq!(pkg.sources[0].format.as_deref(), Some("matpower"));
    assert!(
        pkg.source_maps.iter().any(|e| {
            e.element_path == "/model/balanced_network/buses/0/vm"
                && e.mapping_kind == MappingKind::Exact
                && e.confidence == Confidence::High
                && e.source_ref.record.as_deref() == Some("bus")
                && e.source_ref.field.as_deref() == Some("vm")
        }),
        "expected bus voltage source map: {:?}",
        pkg.source_maps
    );
    assert!(
        pkg.source_maps.iter().any(|e| {
            e.element_path == "/model/balanced_network/branches/0/angmax"
                && e.mapping_kind == MappingKind::Exact
                && e.source_ref.record.as_deref() == Some("branch")
                && e.source_ref.field.as_deref() == Some("angmax")
        }),
        "expected branch angle source map: {:?}",
        pkg.source_maps
    );
    assert_json_roundtrips(&pkg);
}

#[test]
fn matpower_default_frequency_is_not_mapped_as_source_field() {
    let pkg = balanced_package();

    assert!(
        !pkg.source_maps
            .iter()
            .any(|e| e.element_path == "/model/balanced_network/base_frequency"),
        "MATPOWER has no source frequency field: {:?}",
        pkg.source_maps
    );
}

#[test]
fn matpower_loads_and_shunts_map_to_bus_row_fields() {
    let src = "\
function mpc = injections
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t12\t3\t0.5\t0.25\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t10\t2\t30\t-30\t1\t100\t1\t50\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let net = powerio::parse_str(src, "matpower").unwrap().network;
    let pkg = CompilerPackage::from_balanced(net);

    let has_split_bus_field = |path: &str, field: &str| {
        pkg.source_maps.iter().any(|e| {
            e.element_path == path
                && e.mapping_kind == MappingKind::Split
                && e.confidence == Confidence::High
                && e.source_ref.record.as_deref() == Some("bus")
                && e.source_ref.field.as_deref() == Some(field)
        })
    };
    assert!(has_split_bus_field(
        "/model/balanced_network/loads/0/p",
        "p"
    ));
    assert!(has_split_bus_field(
        "/model/balanced_network/loads/0/q",
        "q"
    ));
    assert!(has_split_bus_field(
        "/model/balanced_network/shunts/0/g",
        "g"
    ));
    assert!(has_split_bus_field(
        "/model/balanced_network/shunts/0/b",
        "b"
    ));
    assert!(
        pkg.source_maps.iter().any(|e| {
            e.element_path == "/model/balanced_network/generators/0/pg"
                && e.mapping_kind == MappingKind::Exact
                && e.source_ref.record.as_deref() == Some("generator")
                && e.source_ref.field.as_deref() == Some("pg")
        }),
        "expected generator dispatch source map: {:?}",
        pkg.source_maps
    );
    assert!(
        !pkg.source_maps
            .iter()
            .any(|e| matches!(e.source_ref.record.as_deref(), Some("load" | "shunt"))),
        "MATPOWER injections are bus row fields: {:?}",
        pkg.source_maps
    );
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
fn balanced_origin_matches_source_artifact_kind() {
    let mut net = powerio::parse_str(MATPOWER_SRC, "matpower")
        .expect("parse matpower")
        .network;

    net.source_format = powerio::SourceFormat::Gridfm;
    let gridfm = CompilerPackage::from_balanced(net.clone());
    assert!(matches!(gridfm.origin, Origin::Folder { .. }));
    assert_eq!(gridfm.sources[0].kind, "folder");

    net.source_format = powerio::SourceFormat::PypsaCsv;
    let pypsa = CompilerPackage::from_balanced(net.clone());
    assert!(matches!(pypsa.origin, Origin::Folder { .. }));
    assert_eq!(pypsa.sources[0].kind, "folder");

    net.source_format = powerio::SourceFormat::PowerWorldBinary;
    let pwb = CompilerPackage::from_balanced(net);
    assert!(matches!(pwb.origin, Origin::BinaryFile { .. }));
    assert_eq!(pwb.sources[0].kind, "binary_file");
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
fn future_same_major_schema_version_is_tolerated() {
    let pkg = balanced_package();
    let mut v = serde_json::to_value(&pkg).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("schema_version".to_owned(), serde_json::json!("0.3.0"));
    v.as_object_mut()
        .unwrap()
        .insert("future_field".to_owned(), serde_json::json!({"x": 1}));
    let json = serde_json::to_string(&v).unwrap();

    let back = CompilerPackage::from_json(&json).expect("same major schema version loads");
    assert_eq!(back.schema_version, "0.3.0");
    assert_eq!(back.model_kind(), ModelKind::Balanced);
}

#[test]
fn same_major_prerelease_or_build_schema_version_is_tolerated() {
    for version in ["0.2.0-rc.1", "0.1.0+build.5", "0.3.0-alpha.2+exp"] {
        let pkg = balanced_package();
        let mut v = serde_json::to_value(&pkg).unwrap();
        v.as_object_mut()
            .unwrap()
            .insert("schema_version".to_owned(), serde_json::json!(version));
        let json = serde_json::to_string(&v).unwrap();

        let back = CompilerPackage::from_json(&json)
            .unwrap_or_else(|e| panic!("same-major {version} should load: {e}"));
        assert_eq!(back.schema_version, version);
    }
}

#[test]
fn normalized_solver_table_metadata_records_dense_identities() {
    let net = powerio::parse_str(MATPOWER_WITH_GEN_SRC, "matpower")
        .expect("parse matpower")
        .network;
    let mut pkg = CompilerPackage::from_balanced(net);

    assert!(pkg.attach_normalized_solver_table_metadata().unwrap());

    let meta = pkg
        .derived
        .normalized_solver_tables
        .as_ref()
        .expect("metadata attached");
    assert_eq!(meta.pass, powerio::NORMALIZED_SOLVER_TABLES_PASS);
    assert_eq!(meta.units.power, "per_unit");
    assert_eq!(meta.units.angle, "radian");
    assert_eq!(meta.row_counts.buses, 2);
    assert_eq!(meta.row_counts.loads, 1);
    assert_eq!(meta.row_counts.branches, 1);
    assert_eq!(meta.row_counts.arcs, 2);
    assert_eq!(meta.row_counts.generators, 1);
    assert_eq!(meta.bus_ids, vec![powerio::BusId(1), powerio::BusId(2)]);
    assert_eq!(meta.reference_bus_indices, vec![0]);
    assert_eq!(meta.branch_from_arc_indices, vec![0]);
    assert_eq!(meta.branch_to_arc_indices, vec![1]);
    assert_eq!(meta.source_rows.buses, vec![Some(0), Some(1)]);
    assert_eq!(meta.source_rows.loads, vec![Some(0)]);
    assert_eq!(meta.source_rows.branches, vec![Some(0)]);
    assert_eq!(meta.source_rows.generators, vec![Some(0)]);
    assert_json_roundtrips(&pkg);
}

#[test]
fn incompatible_schema_major_is_rejected() {
    let pkg = balanced_package();
    let mut v = serde_json::to_value(&pkg).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("schema_version".to_owned(), serde_json::json!("1.0.0"));
    let json = serde_json::to_string(&v).unwrap();

    let err = CompilerPackage::from_json(&json).expect_err("major version mismatch must fail");
    assert!(
        err.to_string()
            .contains("unsupported .pio.json schema_version 1.0.0"),
        "{err}"
    );
}

#[test]
fn invalid_schema_version_is_rejected() {
    let pkg = balanced_package();
    for version in [
        "0",
        "0.x.0",
        "0.1.0.1",
        "00.1.0",
        "0.1.0-",
        "0.1.0+",
        "0.1.0-alpha..1",
        "0.1.0+build!",
    ] {
        let mut v = serde_json::to_value(&pkg).unwrap();
        v.as_object_mut()
            .unwrap()
            .insert("schema_version".to_owned(), serde_json::json!(version));
        let json = serde_json::to_string(&v).unwrap();

        let err = CompilerPackage::from_json(&json).expect_err("invalid semver must fail");
        assert!(
            err.to_string()
                .contains(&format!("unsupported .pio.json schema_version {version}")),
            "{err}"
        );
    }
}

#[test]
fn sane_validation_records_balanced_value_domain_findings() {
    let src = "\
function mpc = bad_values
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t0\t0\t230\t1\t1.1\t0.9;
\t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let net = powerio::parse_str(src, "matpower").unwrap().network;
    let mut pkg = CompilerPackage::from_balanced(net);
    pkg.run_sane_validation();

    assert!(
        pkg.diagnostics.iter().any(|d| d.code
            == DiagnosticCode::new("VALIDATE.BALANCED.VALUE_DOMAIN")
            && d.details["field"] == "vm"
            && d.element_path.as_deref() == Some("/model/balanced_network/buses/0/vm")
            && d.source_ref.as_ref().and_then(|r| r.record.as_deref()) == Some("bus")
            && d.source_ref.as_ref().and_then(|r| r.field.as_deref()) == Some("vm")),
        "expected voltage magnitude finding: {:?}",
        pkg.diagnostics
    );
    assert_eq!(pkg.validation.status, ValidationStatus::Warning);
    assert!(
        pkg.validation
            .passes
            .iter()
            .any(|p| p.name == "balanced.value_domain" && p.status == ValidationStatus::Warning),
        "missing balanced value domain pass: {:?}",
        pkg.validation.passes
    );
    assert_json_roundtrips(&pkg);
}

#[test]
fn sane_validation_skips_ambiguous_generator_source_refs() {
    let src = "\
function mpc = duplicate_bad_gens
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t10\t0\t30\t-30\t0\t100\t1\t50\t0;
\t1\t20\t0\t30\t-30\t0\t100\t1\t60\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let net = powerio::parse_str(src, "matpower").unwrap().network;
    let mut pkg = CompilerPackage::from_balanced(net);
    pkg.run_sane_validation();

    let generator_vg: Vec<_> = pkg
        .diagnostics
        .iter()
        .filter(|d| {
            d.code == DiagnosticCode::new("VALIDATE.BALANCED.VALUE_DOMAIN")
                && d.details["element"] == "generator at bus 1"
                && d.details["field"] == "vg"
        })
        .collect();
    assert_eq!(generator_vg.len(), 2, "{:?}", pkg.diagnostics);
    assert!(
        generator_vg.iter().all(|d| d.source_ref.is_none()),
        "ambiguous generator diagnostics must not pick the first row: {generator_vg:?}"
    );
}

#[test]
fn sane_validation_records_multiconductor_structure_findings() {
    use powerio_dist::{DistBus, DistLine, DistNetwork, UntypedObject};

    let mut net = DistNetwork::default();
    net.buses.push(DistBus::new("a", vec!["1".to_owned()]));
    net.lines.push(DistLine::new(
        "l1",
        "a",
        "missing",
        vec!["2".to_owned()],
        vec!["1".to_owned()],
        "missing_code",
        1.0,
    ));
    net.untyped
        .push(UntypedObject::new("regcontrol", "r1", Vec::new()));

    let mut pkg = CompilerPackage::from_multiconductor(net);
    pkg.run_sane_validation();

    for code in [
        "VALIDATE.MULTI.STRUCTURE",
        "VALIDATE.MULTI.TERMINAL_MAP",
        "VALIDATE.MULTI.UNTYPED_OBJECT",
        "VALIDATE.MULTI.NO_VOLTAGE_SOURCE",
    ] {
        assert!(
            pkg.diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::new(code)),
            "missing {code}: {:?}",
            pkg.diagnostics
        );
    }
    assert_eq!(pkg.validation.status, ValidationStatus::Error);
    assert!(
        pkg.validation
            .passes
            .iter()
            .any(|p| p.name == "multiconductor.structure" && p.status == ValidationStatus::Error)
    );
    assert_json_roundtrips(&pkg);
}

#[test]
fn lowering_preflight_accepts_three_phase_without_neutral() {
    let net = preflight_network(&["1", "2", "3"], &[]);
    let report = check_multiconductor_to_balanced_lowering(
        &net,
        powerio_pkg::MulticonductorToBalancedOptions::default(),
    );

    assert_eq!(
        report.convention,
        SequenceTransformConvention::FortescuePowerInvariant
    );
    assert_eq!(report.status, ValidationStatus::Ok);
    assert!(report.is_ready());
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

#[test]
fn lowering_preflight_records_kron_reduction_for_neutral() {
    let net = preflight_network(&["1", "2", "3", "4"], &["4"]);
    let report = check_multiconductor_to_balanced_lowering(
        &net,
        powerio_pkg::MulticonductorToBalancedOptions::default(),
    );

    assert_eq!(report.status, ValidationStatus::Info);
    assert!(report.is_ready());
    assert!(has_lowering_code(
        &report,
        "LOWER.MULTI_TO_BALANCED.KRON_REDUCTION_REQUIRED"
    ));
    assert!(
        report
            .approximations
            .iter()
            .any(|a| a.contains("Kron reduction")),
        "{:?}",
        report.approximations
    );
}

#[test]
fn lowering_preflight_accepts_source_grounded_four_wire_fixture() {
    let text = include_str!("../../tests/data/dist/micro/fourwire_linecode.dss");
    let net = powerio_dist::parse_str(text, "dss").expect("parse four wire fixture");
    let report = check_multiconductor_to_balanced_lowering(
        &net,
        powerio_pkg::MulticonductorToBalancedOptions::default(),
    );

    assert_eq!(report.status, ValidationStatus::Info);
    assert!(report.is_ready(), "{:?}", report.diagnostics);
    assert!(has_lowering_code(
        &report,
        "LOWER.MULTI_TO_BALANCED.KRON_REDUCTION_REQUIRED"
    ));
    assert!(
        !has_lowering_code(&report, "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CONDUCTOR_SET"),
        "{:?}",
        report.diagnostics
    );
}

#[test]
fn lowering_preflight_rejects_one_phase_input() {
    let net = preflight_network(&["1"], &[]);
    let report = check_multiconductor_to_balanced_lowering(
        &net,
        powerio_pkg::MulticonductorToBalancedOptions::default(),
    );

    assert_eq!(report.status, ValidationStatus::Error);
    assert!(!report.is_ready());
    assert!(has_lowering_code(
        &report,
        "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CONDUCTOR_SET"
    ));
}

#[test]
fn lowering_preflight_rejects_two_wire_input() {
    let net = preflight_network(&["1", "2"], &[]);
    let report = check_multiconductor_to_balanced_lowering(
        &net,
        powerio_pkg::MulticonductorToBalancedOptions::default(),
    );

    assert_eq!(report.status, ValidationStatus::Error);
    assert!(!report.is_ready());
    assert!(has_lowering_code(
        &report,
        "LOWER.MULTI_TO_BALANCED.AMBIGUOUS_TERMINAL_MAP"
    ));
}

#[test]
fn lowering_preflight_rejects_untyped_objects() {
    use powerio_dist::UntypedObject;

    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.untyped
        .push(UntypedObject::new("regcontrol", "r1", Vec::new()));
    let report = check_multiconductor_to_balanced_lowering(
        &net,
        powerio_pkg::MulticonductorToBalancedOptions::default(),
    );

    assert_eq!(report.status, ValidationStatus::Error);
    assert!(has_lowering_code(
        &report,
        "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_OBJECT"
    ));
}

#[test]
fn lowering_preflight_rejects_missing_phase_reference() {
    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.sources.clear();
    let report = check_multiconductor_to_balanced_lowering(
        &net,
        powerio_pkg::MulticonductorToBalancedOptions::default(),
    );

    assert_eq!(report.status, ValidationStatus::Error);
    assert!(has_lowering_code(
        &report,
        "LOWER.MULTI_TO_BALANCED.MISSING_PHASE_REFERENCE"
    ));
}

#[test]
fn lowering_preflight_rejects_transformers() {
    use powerio_dist::DistTransformer;

    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.transformers
        .push(DistTransformer::new("t1", Vec::new(), Vec::new(), 3));
    let report = check_multiconductor_to_balanced_lowering(
        &net,
        powerio_pkg::MulticonductorToBalancedOptions::default(),
    );

    assert_eq!(report.status, ValidationStatus::Error);
    assert!(has_lowering_code(
        &report,
        "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_TRANSFORMER"
    ));
}

#[test]
fn package_lowering_preflight_helper_is_read_only() {
    let balanced = balanced_package();
    assert!(
        balanced
            .check_multiconductor_to_balanced_lowering()
            .is_none()
    );

    let pkg = CompilerPackage::from_multiconductor(preflight_network(&["1", "2", "3"], &[]));
    assert!(pkg.lowering_history.is_empty());
    let report = pkg
        .check_multiconductor_to_balanced_lowering()
        .expect("multiconductor package has readiness");
    assert_eq!(report.status, ValidationStatus::Ok);
    assert!(pkg.lowering_history.is_empty());
}

#[test]
fn lowering_produces_balanced_three_phase_without_neutral() {
    let net = preflight_network(&["1", "2", "3"], &[]);
    let lowered =
        lower_multiconductor_to_balanced(&net, MulticonductorToBalancedOptions::default())
            .expect("lower three phase");

    let balanced = lowered.network;
    assert_eq!(balanced.buses.len(), 2);
    assert_eq!(balanced.branches.len(), 1);
    assert_eq!(balanced.loads.len(), 0);
    assert_eq!(balanced.buses[0].kind, powerio::BusType::Ref);
    assert_eq!(balanced.buses[1].kind, powerio::BusType::Pq);
    assert!(balanced.branches[0].x > 0.0);
    assert_eq!(balanced.source_format, powerio::SourceFormat::InMemory);
    assert_eq!(lowered.record.input_kind, ModelKind::Multiconductor);
    assert_eq!(lowered.record.output_kind, ModelKind::Balanced);
    assert_eq!(lowered.record.validation_status, ValidationStatus::Ok);
}

#[test]
fn lowering_produces_balanced_three_phase_with_neutral_kron() {
    let net = preflight_network(&["1", "2", "3", "4"], &["4"]);
    let lowered =
        lower_multiconductor_to_balanced(&net, MulticonductorToBalancedOptions::default())
            .expect("lower four wire");

    assert_eq!(lowered.network.buses.len(), 2);
    assert_eq!(lowered.network.branches.len(), 1);
    assert!(has_diagnostic_code(
        &lowered.record.diagnostics,
        "LOWER.MULTI_TO_BALANCED.KRON_REDUCTION_REQUIRED"
    ));
    assert!(
        lowered
            .record
            .approximations
            .iter()
            .any(|a| a.contains("Kron reduction")),
        "{:?}",
        lowered.record.approximations
    );
}

#[test]
fn lowering_produces_balanced_source_grounded_four_wire_fixture() {
    let text = include_str!("../../tests/data/dist/micro/fourwire_linecode.dss");
    let net = powerio_dist::parse_str(text, "dss").expect("parse four wire fixture");
    let lowered =
        lower_multiconductor_to_balanced(&net, MulticonductorToBalancedOptions::default())
            .expect("lower source grounded four wire fixture");

    assert!(lowered.network.buses.len() >= 2);
    assert_eq!(lowered.network.branches.len(), 1);
    assert_eq!(lowered.network.loads.len(), 3);
    assert!(lowered.network.loads.iter().all(|load| load.p > 0.0));
    assert!(has_diagnostic_code(
        &lowered.record.diagnostics,
        "LOWER.MULTI_TO_BALANCED.KRON_REDUCTION_REQUIRED"
    ));
}

#[test]
fn lowering_rejects_one_phase_input() {
    assert_lowering_rejects(
        &preflight_network(&["1"], &[]),
        "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CONDUCTOR_SET",
    );
}

#[test]
fn lowering_rejects_two_wire_input() {
    assert_lowering_rejects(
        &preflight_network(&["1", "2"], &[]),
        "LOWER.MULTI_TO_BALANCED.AMBIGUOUS_TERMINAL_MAP",
    );
}

#[test]
fn lowering_rejects_missing_phase_reference() {
    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.sources.clear();
    assert_lowering_rejects(&net, "LOWER.MULTI_TO_BALANCED.MISSING_PHASE_REFERENCE");
}

#[test]
fn lowering_rejects_transformer_input() {
    use powerio_dist::DistTransformer;

    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.transformers
        .push(DistTransformer::new("t1", Vec::new(), Vec::new(), 3));
    assert_lowering_rejects(&net, "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_TRANSFORMER");
}

#[test]
fn lowering_rejects_untyped_object_input() {
    use powerio_dist::UntypedObject;

    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.untyped
        .push(UntypedObject::new("regcontrol", "r1", Vec::new()));
    assert_lowering_rejects(&net, "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_OBJECT");
}

#[test]
fn lowering_rejects_closed_switch_input() {
    use powerio_dist::DistSwitch;

    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.switches.push(DistSwitch::new(
        "sw1",
        "sourcebus",
        "loadbus",
        strings(&["1", "2", "3"]),
        strings(&["1", "2", "3"]),
        false,
    ));
    assert_lowering_rejects(&net, "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CLOSED_SWITCH");
}

#[test]
fn lowering_rejects_generator_unknown_bus() {
    use powerio_dist::{Configuration, DistGenerator};

    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.generators.push(DistGenerator::new(
        "g_missing",
        "missing",
        strings(&["1", "2", "3"]),
        Configuration::Wye,
        vec![1_000.0, 1_000.0, 1_000.0],
        vec![0.0, 0.0, 0.0],
    ));

    assert_lowering_rejects(&net, "LOWER.MULTI_TO_BALANCED.UNKNOWN_BUS");
}

#[test]
fn lowering_preserves_single_phase_shunt_total() {
    use powerio_dist::DistShunt;

    let mut net = preflight_network(&["1", "2", "3"], &[]);
    net.shunts.push(DistShunt::new(
        "s1",
        "loadbus",
        strings(&["1"]),
        vec![vec![0.03]],
        vec![vec![0.06]],
    ));

    let lowered =
        lower_multiconductor_to_balanced(&net, MulticonductorToBalancedOptions::default())
            .expect("lower single phase shunt");
    assert_eq!(lowered.network.shunts.len(), 1);

    let expected_g = 0.03 * 240.0 * 240.0 / 1_000_000.0;
    let expected_b = 0.06 * 240.0 * 240.0 / 1_000_000.0;
    let shunt = &lowered.network.shunts[0];
    assert!(
        (shunt.g - expected_g).abs() < 1.0e-12,
        "got {}, expected {}",
        shunt.g,
        expected_g
    );
    assert!(
        (shunt.b - expected_b).abs() < 1.0e-12,
        "got {}, expected {}",
        shunt.b,
        expected_b
    );
}

#[test]
fn package_lowering_returns_derived_balanced_package() {
    let mut parent =
        CompilerPackage::from_multiconductor(preflight_network(&["1", "2", "3", "4"], &["4"]));
    parent.push_lowering(powerio_pkg::LoweringRecord::new(
        "previous-pass",
        ModelKind::Multiconductor,
        ModelKind::Multiconductor,
    ));
    let lowered = parent
        .lower_multiconductor_to_balanced(MulticonductorToBalancedOptions::default())
        .expect("lower package");

    assert_eq!(lowered.model_kind(), ModelKind::Balanced);
    assert!(lowered.as_balanced().is_some());
    assert!(lowered.as_multiconductor().is_none());
    match &lowered.origin {
        Origin::Derived { pass, .. } => assert_eq!(pass, "multiconductor-to-balanced"),
        other => panic!("expected derived origin, got {other:?}"),
    }
    assert_eq!(lowered.lowering_history.len(), 2);
    assert_eq!(
        lowered.lowering_history[1].pass,
        "multiconductor-to-balanced"
    );
    assert!(has_diagnostic_code(
        &lowered.diagnostics,
        "LOWER.MULTI_TO_BALANCED.KRON_REDUCTION_REQUIRED"
    ));
    assert!(
        lowered
            .source_maps
            .iter()
            .any(|entry| entry.mapping_kind == MappingKind::Synthetic),
        "missing synthetic provenance: {:?}",
        lowered.source_maps
    );
    assert!(
        lowered
            .source_maps
            .iter()
            .any(|entry| entry.mapping_kind == MappingKind::ConvertedUnits),
        "missing unit conversion provenance: {:?}",
        lowered.source_maps
    );
    assert!(
        lowered
            .validation
            .passes
            .iter()
            .any(|pass| pass.name == "balanced.structure" && pass.status == ValidationStatus::Ok),
        "balanced sane validation did not run: {:?}",
        lowered.validation.passes
    );
    assert_json_roundtrips(&lowered);
}

#[test]
fn package_lowering_rejects_balanced_package() {
    let err = balanced_package()
        .lower_multiconductor_to_balanced(MulticonductorToBalancedOptions::default())
        .expect_err("balanced package is not accepted");
    assert!(has_diagnostic_code(
        &err.diagnostics,
        "LOWER.MULTI_TO_BALANCED.WRONG_MODEL_KIND"
    ));
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
    use powerio_dist::{Configuration, DistLoad, DistLoadVoltageModel, DistNetwork};

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
    let mut load = DistLoad::new(
        "l1",
        "b1",
        vec![
            "a".to_owned(),
            "b".to_owned(),
            "c".to_owned(),
            "n".to_owned(),
        ],
        Configuration::Wye,
        vec![100.0, 100.0, 100.0],
        vec![30.0, 30.0, 30.0],
    );
    load.voltage_model = zip.clone();
    net.loads.push(load);

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

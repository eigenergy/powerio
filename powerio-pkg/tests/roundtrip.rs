//! Serde round-trip and invariant tests for the `.pio.json` compiler package.

use std::collections::BTreeMap;

use powerio_pkg::{
    Confidence, DiagnosticCode, DiagnosticSeverity, DiagnosticStage, ElementRef, ElementUpdate,
    MappingKind, ModelKind, MulticonductorToBalancedOptions, MulticonductorToBalancedReadiness,
    NetworkPackage, OperatingPoint, OperatingPointSeries, Origin, READ_TRANSMISSION_PARSE_WARNING,
    SequenceTransformConvention, SourceDescriptor, SourceMapEntry, SourceRef, StructuredDiagnostic,
    StudyBlock, StudyCommit, StudyEdit, TimeAxis, ValidationStatus,
    check_multiconductor_to_balanced_lowering, ensure_payload_uids,
    lower_multiconductor_to_balanced,
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

fn balanced_package() -> NetworkPackage {
    let net = powerio::parse_str(MATPOWER_SRC, "matpower")
        .expect("parse matpower")
        .network;
    NetworkPackage::from_balanced(net)
}

fn multiconductor_package() -> NetworkPackage {
    // A bare circuit materializes a vsource with several defaulted fields, which
    // exercises the defaulted -> source-map lift.
    let net = powerio_dist::parse_str("New Circuit.c1", "dss").expect("parse dss");
    NetworkPackage::from_multiconductor(net)
}

fn balanced_package_with_gen() -> NetworkPackage {
    let mut net = powerio::parse_str(MATPOWER_WITH_GEN_SRC, "matpower")
        .expect("parse matpower with gen")
        .network;
    // Source uids the sample operating point updates resolve against; every
    // other row gets a synthesized `{table}:{row}` uid at package build.
    net.loads[0].uid = Some("load_1".to_owned());
    net.generators[0].uid = Some("gen_1".to_owned());
    net.branches[0].uid = Some("branch_1".to_owned());
    NetworkPackage::from_balanced(net)
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
    point0.updates.push(ElementUpdate::new(
        ElementRef::new("loads", 0).with_source_uid("load_1"),
        fields(&[
            ("p", serde_json::json!(12.0)),
            ("q", serde_json::json!(6.0)),
        ]),
    ));

    let mut point1 = OperatingPoint::new(1);
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

fn study_commit(edits: Vec<StudyEdit>) -> StudyCommit {
    let mut commit = StudyCommit::default();
    commit.edits = edits;
    commit
}

fn study_block(commits: Vec<StudyCommit>) -> StudyBlock {
    let mut study = StudyBlock::default();
    study.commits = commits;
    study
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

fn preflight_network(terminals: &[&str], grounded: &[&str]) -> powerio_dist::MulticonductorNetwork {
    use powerio_dist::{DistBus, DistLine, DistLineCode, MulticonductorNetwork, VoltageSource};

    let n = terminals.len();
    let terminal_map = strings(terminals);
    let (v_magnitude, v_angle) = phase_reference(terminals, grounded);
    let mut net = MulticonductorNetwork::default();
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

fn assert_lowering_rejects(net: &powerio_dist::MulticonductorNetwork, code: &str) {
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
fn assert_json_roundtrips(pkg: &NetworkPackage) {
    let json1 = pkg.to_json_pretty().expect("serialize");
    let back = NetworkPackage::from_json(&json1).expect("deserialize");
    let json2 = back.to_json_pretty().expect("re-serialize");
    assert_eq!(json1, json2, "package JSON is not round-trip stable");
}

#[test]
fn powerio_version_is_present_and_required() {
    let pkg = balanced_package();
    assert_eq!(pkg.powerio_version, powerio::VERSION);

    // A document without the field is refused. Defaulting it to the current
    // version would let a package from an older lineage skip the gate by
    // dropping the field: every payload difference between the two lineages
    // would then arrive as a serde default, with no error and no warning.
    let mut v = serde_json::to_value(&pkg).unwrap();
    v.as_object_mut().unwrap().remove("powerio_version");
    let err = NetworkPackage::from_json(&serde_json::to_string(&v).unwrap()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("before powerio 0.9.0"), "got: {msg}");
    assert!(msg.contains("regenerate"), "got: {msg}");
}

#[test]
fn version_gate_rejects_other_lineages_and_says_regenerate() {
    let pkg = balanced_package();
    let mut v = serde_json::to_value(&pkg).unwrap();
    let (major, minor) = lineage(powerio::VERSION);
    assert_eq!(major, 0, "update this test at 1.0.0");

    // A file from the previous minor is rejected with an error naming this
    // build's lineage and the remedy. While the major is 0 a minor bump is
    // incompatible, which is what cargo and Pkg already mean by 0.x.
    v["powerio_version"] = serde_json::json!(format!("0.{}.1", minor - 1));
    let err = NetworkPackage::from_json(&serde_json::to_string(&v).unwrap()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains(&format!("0.{}.1", minor - 1)), "got: {msg}");
    assert!(msg.contains(&format!("0.{minor}.x")), "got: {msg}");
    assert!(msg.contains("regenerate"), "got: {msg}");

    // Same lineage, additive patch: loads.
    v["powerio_version"] = serde_json::json!(format!("0.{minor}.99"));
    NetworkPackage::from_json(&serde_json::to_string(&v).unwrap()).unwrap();

    // Later lineages and garbage: rejected.
    for bad in [
        format!("0.{}.0", minor + 1),
        "1.0.0".into(),
        "not-semver".into(),
    ] {
        v["powerio_version"] = serde_json::json!(bad);
        NetworkPackage::from_json(&serde_json::to_string(&v).unwrap()).unwrap_err();
    }

    // Fields an older lineage wrote are ignored as unknown, as any unknown top
    // level field from another producer is; the version gate is the only
    // arbiter.
    v["powerio_version"] = serde_json::json!(powerio::VERSION);
    v["schema_version"] = serde_json::json!("0.2.1");
    v["payload_schema"] = serde_json::json!("https://powerio.dev/schema/pio-payload-balanced/1");
    v["payload_schema_version"] = serde_json::json!("1.1.0");
    NetworkPackage::from_json(&serde_json::to_string(&v).unwrap()).unwrap();
}

/// The `(major, minor)` of a semver string, for tests that must express a
/// neighbouring lineage without hardcoding this release's number.
fn lineage(version: &str) -> (u64, u64) {
    let core = version.split(['-', '+']).next().unwrap();
    let mut parts = core.split('.');
    let major = parts.next().unwrap().parse().unwrap();
    let minor = parts.next().unwrap().parse().unwrap();
    (major, minor)
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
    let back = NetworkPackage::from_json(&json).unwrap();
    assert_eq!(back.as_balanced().unwrap().buses.len(), 2);
    assert_eq!(back.as_balanced().unwrap().branches.len(), 1);
}

#[test]
fn goc3_package_operating_points_materialize_static_snapshots() {
    let parsed = powerio::parse_str(GOC3_PACKAGE_SRC, "goc3-json").expect("parse goc3");
    let net = &parsed.network;
    assert_eq!(net.generators.len(), 1);
    assert_eq!(net.loads.len(), 1);
    assert_close(net.generators[0].pmax, 100.0);
    assert_close(net.loads[0].p, 40.0);

    let pkg = NetworkPackage::from_parsed_balanced(parsed);
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
fn balanced_package_constructor_does_not_run_source_adapters() {
    let parsed = powerio::parse_str(GOC3_PACKAGE_SRC, "goc3-json").expect("parse goc3");
    let pkg = NetworkPackage::from_balanced(parsed.network);
    assert!(pkg.operating_points().is_none());
}

/// The operating point series derives from the document the reader already
/// parsed (`Parsed::document`), never from a second parse of the retained
/// source text: stripping the text must not change the outcome.
#[test]
fn goc3_operating_points_derive_from_the_reader_parse() {
    let mut parsed = powerio::parse_str(GOC3_PACKAGE_SRC, "goc3-json").expect("parse goc3");
    assert!(matches!(
        parsed.document,
        Some(powerio::SourceDocument::Goc3(_))
    ));
    parsed.network.source = None;
    let pkg = NetworkPackage::from_parsed_balanced(parsed);
    assert!(pkg.operating_points().is_some());
}

#[test]
fn goc3_oversized_time_periods_is_refused_not_allocated() {
    // `time_periods` sizes the per-period point and label vectors; an oversized
    // value that does not match the interval_duration array would otherwise
    // drive an unbounded up-front allocation. The mismatch is refused, so the
    // package is static-only with a diagnostic instead of aborting.
    let src = GOC3_PACKAGE_SRC.replace(
        r#""time_periods": 2, "interval_duration": [1.0, 2.0]"#,
        r#""time_periods": 999999999999999999, "interval_duration": [1.0, 2.0]"#,
    );
    let parsed = powerio::parse_str(&src, "goc3-json").expect("parse goc3");
    let pkg = NetworkPackage::from_parsed_balanced(parsed);
    assert!(
        pkg.operating_points().is_none(),
        "an inconsistent time_periods must not yield a series"
    );
}

#[test]
fn goc3_operating_points_follow_parser_row_assignment() {
    // A device without a uid still occupies a payload row; the extractor
    // must keep counting so later devices' updates land on their own rows,
    // and the parent's package_id must not leak into the derived package.
    let src = GOC3_PACKAGE_SRC.replacen(
        r#"{"uid": "prod", "bus": "bus_00""#,
        r#"{"bus": "bus_00", "device_type": "producer", "initial_status": {"on_status": 1, "p": 0.0, "q": 0.0}},
      {"uid": "prod", "bus": "bus_00""#,
        1,
    );
    let parsed = powerio::parse_str(&src, "goc3-json").expect("parse goc3");
    assert_eq!(parsed.network.generators.len(), 2);

    let pkg = NetworkPackage::from_parsed_balanced(parsed).with_package_id("parent");
    let series = pkg.operating_points().expect("operating points");
    let update = &series.points[1].updates[0];
    assert_eq!(update.element.table, "generators");
    assert_eq!(
        update.element.row,
        Some(1),
        "uid-less producer occupies row 0"
    );
    assert_eq!(update.element.source_uid.as_deref(), Some("prod"));

    let materialized = pkg.materialize_operating_point(1).expect("materialize");
    let balanced = materialized.as_balanced().expect("balanced payload");
    // Row 0 (the uid-less device) keeps its static bounds; row 1 gets the
    // period 1 update.
    assert_close(balanced.generators[0].pmax, 0.0);
    assert_close(balanced.generators[1].pmax, 80.0);
    assert_eq!(materialized.package_id, None);
    match &materialized.origin {
        powerio_pkg::Origin::Derived {
            parent_package_id, ..
        } => assert_eq!(parent_package_id.as_deref(), Some("parent")),
        other => panic!("expected derived origin, got {other:?}"),
    }
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
    let back = NetworkPackage::from_json(&json).unwrap();
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

    let back = NetworkPackage::from_json(&pkg.to_json_pretty().unwrap()).unwrap();
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
            .contains("operating point table `loads` has no row 99")
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

    let err = NetworkPackage::from_json(&json).expect_err("kind mismatch must be rejected");
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
    let back = NetworkPackage::from_json(&json).unwrap();
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
    let back = NetworkPackage::from_json(&json).unwrap();
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
    let pkg = NetworkPackage::from_balanced(net);

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
    let in_mem = NetworkPackage::from_balanced(powerio::BalancedNetwork::in_memory(
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
    let gridfm = NetworkPackage::from_balanced(net.clone());
    assert!(matches!(gridfm.origin, Origin::Folder { .. }));
    assert_eq!(gridfm.sources[0].kind, "folder");

    net.source_format = powerio::SourceFormat::PypsaCsv;
    let pypsa = NetworkPackage::from_balanced(net.clone());
    assert!(matches!(pypsa.origin, Origin::Folder { .. }));
    assert_eq!(pypsa.sources[0].kind, "folder");

    net.source_format = powerio::SourceFormat::PowerWorldBinary;
    let pwb = NetworkPackage::from_balanced(net);
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
    let back = NetworkPackage::from_json(&json).expect("tolerate unknown field");
    assert_eq!(back.model_kind(), ModelKind::Balanced);
    assert!(back.kind_is_consistent());
    assert_eq!(back.as_balanced().unwrap().buses.len(), 2);
}

#[test]
fn a_future_patch_of_this_lineage_is_tolerated() {
    // A newer patch in the reader's lineage with a field this reader does not
    // know: both are additive, so the document loads.
    let (_, minor) = lineage(powerio::VERSION);
    let future = format!("0.{minor}.99");
    let pkg = balanced_package();
    let mut v = serde_json::to_value(&pkg).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("powerio_version".to_owned(), serde_json::json!(future));
    v.as_object_mut()
        .unwrap()
        .insert("future_field".to_owned(), serde_json::json!({"x": 1}));
    let json = serde_json::to_string(&v).unwrap();

    let back = NetworkPackage::from_json(&json).expect("same lineage patch loads");
    assert_eq!(back.powerio_version, future);
    assert_eq!(back.model_kind(), ModelKind::Balanced);
}

#[test]
fn a_prerelease_or_build_tag_of_this_lineage_is_tolerated() {
    let (_, minor) = lineage(powerio::VERSION);
    for suffix in ["-rc.1", "+build.5", "-alpha.2+exp"] {
        let version = format!("0.{minor}.0{suffix}");
        let pkg = balanced_package();
        let mut v = serde_json::to_value(&pkg).unwrap();
        v.as_object_mut()
            .unwrap()
            .insert("powerio_version".to_owned(), serde_json::json!(version));
        let json = serde_json::to_string(&v).unwrap();

        let back = NetworkPackage::from_json(&json)
            .unwrap_or_else(|e| panic!("same-lineage {version} should load: {e}"));
        assert_eq!(back.powerio_version, version);
    }
}

#[test]
fn normalized_solver_table_metadata_records_dense_identities() {
    let net = powerio::parse_str(MATPOWER_WITH_GEN_SRC, "matpower")
        .expect("parse matpower")
        .network;
    let mut pkg = NetworkPackage::from_balanced(net);

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
fn an_incompatible_major_is_rejected() {
    let pkg = balanced_package();
    let mut v = serde_json::to_value(&pkg).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("powerio_version".to_owned(), serde_json::json!("1.0.0"));
    let json = serde_json::to_string(&v).unwrap();

    let err = NetworkPackage::from_json(&json).expect_err("major version mismatch must fail");
    let msg = err.to_string();
    assert!(msg.contains("`powerio_version` 1.0.0"), "{msg}");
    assert!(msg.contains("regenerate"), "{msg}");
}

#[test]
fn a_version_that_is_not_semver_is_rejected() {
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
            .insert("powerio_version".to_owned(), serde_json::json!(version));
        let json = serde_json::to_string(&v).unwrap();

        let err = NetworkPackage::from_json(&json).expect_err("invalid semver must fail");
        assert!(
            err.to_string()
                .contains(&format!("`powerio_version` {version}")),
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
    let mut pkg = NetworkPackage::from_balanced(net);
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
    let mut pkg = NetworkPackage::from_balanced(net);
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
    use powerio_dist::{DistBus, DistLine, MulticonductorNetwork, UntypedObject};

    let mut net = MulticonductorNetwork::default();
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

    let mut pkg = NetworkPackage::from_multiconductor(net);
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

    let pkg = NetworkPackage::from_multiconductor(preflight_network(&["1", "2", "3"], &[]));
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
        NetworkPackage::from_multiconductor(preflight_network(&["1", "2", "3", "4"], &["4"]));
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
    let back = NetworkPackage::from_json(&pkg.to_json_pretty().unwrap()).unwrap();
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
    use powerio_dist::{Configuration, DistLoad, DistLoadVoltageModel, MulticonductorNetwork};

    let zip = DistLoadVoltageModel::Zip {
        v_nom: vec![230.0, 230.0, 230.0],
        alpha_z: vec![0.5, 0.5, 0.5],
        alpha_i: vec![0.2, 0.2, 0.2],
        alpha_p: vec![0.3, 0.3, 0.3],
        beta_z: vec![0.4, 0.4, 0.4],
        beta_i: vec![0.3, 0.3, 0.3],
        beta_p: vec![0.3, 0.3, 0.3],
    };
    let mut net = MulticonductorNetwork::default();
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

    let pkg = NetworkPackage::from_multiconductor(net);
    assert_eq!(pkg.model_kind(), ModelKind::Multiconductor);
    assert_json_roundtrips(&pkg);

    let back = NetworkPackage::from_json(&pkg.to_json_pretty().unwrap()).unwrap();
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

// --- row identity ----------------------------------------------------------

fn single_point_series(point: OperatingPoint) -> OperatingPointSeries {
    OperatingPointSeries::new(TimeAxis::new(1).with_duration_hours(vec![1.0]), vec![point])
}

#[test]
fn package_synthesizes_row_identity() {
    let pkg = balanced_package();
    // MATPOWER has no source uids, so every row gets a synthesized identity.
    let net = pkg.as_balanced().unwrap();
    assert_eq!(net.buses[0].uid.as_deref(), Some("buses:0"));
    assert_eq!(net.buses[1].uid.as_deref(), Some("buses:1"));
    assert_eq!(net.branches[0].uid.as_deref(), Some("branches:0"));

    let v = serde_json::to_value(&pkg).unwrap();
    assert_eq!(v["powerio_version"], serde_json::json!(powerio::VERSION));
    assert_eq!(
        v["model"]["balanced_network"]["buses"][0]["uid"],
        serde_json::json!("buses:0")
    );
    assert_json_roundtrips(&pkg);
}

#[test]
fn duplicate_payload_uid_is_diagnosed_without_operating_points() {
    // A source-supplied uid equal to the `{table}:{row}` value
    // ensure_payload_uids mints for a uid-less sibling collides at build;
    // validation must surface the ambiguity even when nothing (no operating
    // points, no study) references it yet.
    let mut net = powerio::parse_str(MATPOWER_SRC, "matpower")
        .unwrap()
        .network;
    net.buses[1].uid = Some("buses:0".to_owned());
    let mut pkg = NetworkPackage::from_balanced(net);
    pkg.run_sane_validation();

    assert!(pkg.operating_points().is_none());
    assert!(
        pkg.diagnostics.iter().any(|d| {
            d.code == DiagnosticCode::new("VALIDATE.BALANCED.PAYLOAD_IDENTITY")
                && d.message.contains("`buses:0`")
                && d.element_path.as_deref() == Some("/model/balanced_network/buses/1/uid")
        }),
        "expected duplicate uid diagnostic: {:?}",
        pkg.diagnostics
    );
    assert_eq!(pkg.validation.status, ValidationStatus::Error);
    assert!(
        pkg.validation
            .passes
            .iter()
            .any(|p| p.name == "balanced.payload_identity" && p.status == ValidationStatus::Error),
        "missing payload identity pass: {:?}",
        pkg.validation.passes
    );
    assert_json_roundtrips(&pkg);

    // Unique uids leave the pass green, so the check itself is visible in the
    // validation summary of every balanced package.
    let mut clean = NetworkPackage::from_balanced(
        powerio::parse_str(MATPOWER_SRC, "matpower")
            .unwrap()
            .network,
    );
    clean.run_sane_validation();
    assert!(
        clean
            .validation
            .passes
            .iter()
            .any(|p| p.name == "balanced.payload_identity" && p.status == ValidationStatus::Ok),
        "missing payload identity pass: {:?}",
        clean.validation.passes
    );
}

#[test]
fn geo_types_share_the_same_json_shape() {
    let balanced_location = powerio::Location {
        x: -80.0,
        y: 35.0,
        kind: Some(powerio::CoordsKind::Manual),
    };
    let dist_location = powerio_dist::Location {
        x: -80.0,
        y: 35.0,
        kind: Some(powerio_dist::CoordsKind::Manual),
    };
    assert_eq!(
        serde_json::to_value(balanced_location).unwrap(),
        serde_json::to_value(dist_location).unwrap()
    );

    let balanced_geo = powerio::GeoMeta {
        space: powerio::CoordinateSpace::Geographic { crs: None },
        kind: Some(powerio::CoordsKind::Source),
    };
    let dist_geo = powerio_dist::GeoMeta {
        space: powerio_dist::CoordinateSpace::Geographic { crs: None },
        kind: Some(powerio_dist::CoordsKind::Source),
    };
    let expected = serde_json::json!({"space": "geographic", "kind": "source"});
    assert_eq!(serde_json::to_value(&balanced_geo).unwrap(), expected);
    assert_eq!(
        serde_json::to_value(balanced_geo).unwrap(),
        serde_json::to_value(dist_geo).unwrap()
    );

    let empty_dist = powerio_dist::MulticonductorNetwork::default();
    let v = serde_json::to_value(empty_dist).unwrap();
    assert!(v.get("geo").is_none());
    let bus = powerio_dist::DistBus::new("b1", vec!["1".to_owned()]);
    let v = serde_json::to_value(bus).unwrap();
    assert!(v.get("location").is_none());
}

#[test]
fn identity_only_update_resolves_without_row() {
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::by_source_uid("loads", "load_1"),
        fields(&[("p", serde_json::json!(33.0))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(single_point_series(point));

    // The wire form omits `row` entirely.
    let v = serde_json::to_value(&pkg).unwrap();
    let element = &v["operating_points"]["points"][0]["updates"][0]["element"];
    assert!(element.get("row").is_none());
    assert_eq!(element["source_uid"], serde_json::json!("load_1"));

    let back = NetworkPackage::from_json(&pkg.to_json_pretty().unwrap()).unwrap();
    let materialized = back.materialize_operating_point(0).unwrap();
    assert_close(materialized.as_balanced().unwrap().loads[0].p, 33.0);
}

#[test]
fn row_identity_mismatch_is_rejected() {
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::new("generators", 7).with_source_uid("gen_1"),
        fields(&[("pg", serde_json::json!(1.0))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(single_point_series(point));
    let err = pkg
        .materialize_operating_point(0)
        .expect_err("row/uid mismatch must fail");
    assert!(
        err.to_string()
            .contains("names uid `gen_1` (row 0) but carries row 7"),
        "{err}"
    );
}

#[test]
fn unknown_identity_is_rejected_and_reported_by_validation() {
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::by_source_uid("loads", "nope"),
        fields(&[("p", serde_json::json!(1.0))]),
    ));
    let mut pkg = balanced_package_with_gen().with_operating_points(single_point_series(point));
    let err = pkg
        .materialize_operating_point(0)
        .expect_err("unknown identity must fail");
    assert!(err.to_string().contains("unknown identity"), "{err}");

    // The same finding surfaces through validation without materializing.
    pkg.run_sane_validation();
    assert_eq!(pkg.validation.status, ValidationStatus::Error);
    let diagnostic = pkg
        .diagnostics
        .iter()
        .find(|d| d.code.as_str() == "VALIDATE.PACKAGE.OPERATING_IDENTITY")
        .expect("identity diagnostic");
    assert_eq!(
        diagnostic.element_path.as_deref(),
        Some("/operating_points/points/0/updates/0")
    );
}

#[test]
fn duplicate_payload_identities_are_rejected() {
    let mut net = powerio::parse_str(MATPOWER_SRC, "matpower")
        .expect("parse matpower")
        .network;
    net.buses[0].uid = Some("dup".to_owned());
    net.buses[1].uid = Some("dup".to_owned());
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::by_source_uid("buses", "dup"),
        fields(&[("vm", serde_json::json!(1.02))]),
    ));
    let pkg = NetworkPackage::from_balanced(net).with_operating_points(single_point_series(point));
    let err = pkg
        .materialize_operating_point(0)
        .expect_err("duplicate identity must fail");
    assert!(err.to_string().contains("more than one row"), "{err}");
}

#[test]
fn update_must_not_overwrite_uid() {
    let mut point = OperatingPoint::new(0);
    point.updates.push(ElementUpdate::new(
        ElementRef::by_source_uid("loads", "load_1"),
        fields(&[("uid", serde_json::json!("other"))]),
    ));
    let pkg = balanced_package_with_gen().with_operating_points(single_point_series(point));
    let err = pkg
        .materialize_operating_point(0)
        .expect_err("uid overwrite must fail");
    assert!(
        err.to_string().contains("must not overwrite `uid`"),
        "{err}"
    );
}

#[test]
fn payload_without_uids_keeps_row_semantics() {
    // A minimal document: payload rows carry no uids while updates still carry
    // advisory source_uid values. The wire row must keep addressing alone.
    let pkg = balanced_package_with_gen().with_operating_points(sample_operating_points());
    let mut v = serde_json::to_value(&pkg).unwrap();
    let payload = v["model"]["balanced_network"].as_object_mut().unwrap();
    for table in [
        "buses",
        "loads",
        "shunts",
        "branches",
        "switches",
        "generators",
        "storage",
        "hvdc",
        "transformers_3w",
    ] {
        if let Some(rows) = payload.get_mut(table).and_then(|t| t.as_array_mut()) {
            for row in rows {
                row.as_object_mut().unwrap().remove("uid");
            }
        }
    }

    let bare = NetworkPackage::from_json(&v.to_string()).unwrap();
    let materialized = bare.materialize_operating_point(0).unwrap();
    assert_close(materialized.as_balanced().unwrap().loads[0].p, 12.0);
}

#[test]
fn element_ref_wire_requires_row_or_identity() {
    let err = serde_json::from_str::<ElementRef>(r#"{"table": "loads"}"#)
        .expect_err("neither row nor identity");
    assert!(err.to_string().contains("needs `row` or `source_uid`"));

    let by_uid: ElementRef =
        serde_json::from_str(r#"{"table": "loads", "source_uid": "l1"}"#).unwrap();
    assert_eq!(by_uid, ElementRef::by_source_uid("loads", "l1"));
    assert_eq!(by_uid.row, None);

    let by_row: ElementRef = serde_json::from_str(r#"{"table": "loads", "row": 3}"#).unwrap();
    assert_eq!(by_row, ElementRef::new("loads", 3));
    assert_eq!(by_row.row, Some(3));
}

#[test]
#[cfg(feature = "schema")]
fn element_ref_schema_requires_non_null_row_or_identity() {
    let schema = serde_json::to_value(schemars::schema_for!(ElementRef)).unwrap();
    let any_of = schema
        .get("anyOf")
        .and_then(serde_json::Value::as_array)
        .expect("ElementRef schema has anyOf");

    assert!(any_of.iter().any(|entry| {
        entry.get("required") == Some(&serde_json::json!(["row"]))
            && entry.pointer("/properties/row/type") == Some(&serde_json::json!("integer"))
    }));
    assert!(any_of.iter().any(|entry| {
        entry.get("required") == Some(&serde_json::json!(["source_uid"]))
            && entry.pointer("/properties/source_uid/type") == Some(&serde_json::json!("string"))
    }));
}

#[test]
fn goc3_updates_resolve_by_identity_not_row_order() {
    // The uid-less producer fixture from the row assignment test: `prod` is
    // payload row 1 and row 0 is a synthesized identity. Identity alone finds
    // the right row; a contradicting row or an unknown uid fails.
    let src = GOC3_PACKAGE_SRC.replacen(
        r#"{"uid": "prod", "bus": "bus_00""#,
        r#"{"bus": "bus_00", "device_type": "producer", "initial_status": {"on_status": 1, "p": 0.0, "q": 0.0}},
      {"uid": "prod", "bus": "bus_00""#,
        1,
    );
    let net = powerio::parse_str(&src, "goc3-json")
        .expect("parse goc3")
        .network;
    assert_eq!(net.generators[1].uid.as_deref(), Some("prod"));
    let pkg = NetworkPackage::from_balanced(net);
    let balanced = pkg.as_balanced().unwrap();
    assert_eq!(balanced.generators[0].uid.as_deref(), Some("generators:0"));
    assert_eq!(balanced.buses[0].uid.as_deref(), Some("bus_00"));

    let mut by_identity = OperatingPoint::new(0);
    by_identity.updates.push(ElementUpdate::new(
        ElementRef::by_source_uid("generators", "prod"),
        fields(&[("pmax", serde_json::json!(123.0))]),
    ));
    let materialized = pkg
        .clone()
        .with_operating_points(single_point_series(by_identity))
        .materialize_operating_point(0)
        .expect("identity resolves");
    let updated = materialized.as_balanced().unwrap();
    assert_close(updated.generators[1].pmax, 123.0);
    assert_close(updated.generators[0].pmax, 0.0);

    let mut wrong_row = OperatingPoint::new(0);
    wrong_row.updates.push(ElementUpdate::new(
        ElementRef::new("generators", 0).with_source_uid("prod"),
        fields(&[("pmax", serde_json::json!(123.0))]),
    ));
    let err = pkg
        .clone()
        .with_operating_points(single_point_series(wrong_row))
        .materialize_operating_point(0)
        .expect_err("row/uid mismatch must fail");
    assert!(
        err.to_string()
            .contains("names uid `prod` (row 1) but carries row 0"),
        "{err}"
    );

    let mut unknown = OperatingPoint::new(0);
    unknown.updates.push(ElementUpdate::new(
        ElementRef::by_source_uid("generators", "ghost"),
        fields(&[("pmax", serde_json::json!(123.0))]),
    ));
    let err = pkg
        .with_operating_points(single_point_series(unknown))
        .materialize_operating_point(0)
        .expect_err("unknown uid must fail");
    assert!(err.to_string().contains("unknown identity"), "{err}");
}

#[test]
fn package_balanced_reader_warnings_become_diagnostics() {
    let parsed = powerio::parse_str(MATPOWER_SRC, "matpower").expect("parse matpower");
    let mut pkg = NetworkPackage::from_balanced_with_read_warnings(
        parsed.network,
        READ_TRANSMISSION_PARSE_WARNING,
        vec!["ignored source table".to_owned()],
    );

    assert!(pkg.diagnostics.iter().any(|d| {
        d.code.as_str() == READ_TRANSMISSION_PARSE_WARNING
            && d.severity == DiagnosticSeverity::Warning
            && d.stage == DiagnosticStage::Read
            && d.message == "ignored source table"
    }));

    pkg.run_sane_validation();
    assert!(
        pkg.diagnostics
            .iter()
            .any(|d| d.code.as_str() == READ_TRANSMISSION_PARSE_WARNING),
        "reader warning diagnostic was dropped by validation"
    );
}

#[test]
fn ensure_payload_uids_is_public_and_deterministic() {
    let mut net = powerio::parse_str(MATPOWER_WITH_GEN_SRC, "matpower")
        .expect("parse matpower")
        .network;
    net.buses[0].uid = Some("source-bus".to_owned());
    net.loads[0].uid = Some("source-load".to_owned());

    ensure_payload_uids(&mut net);
    let first = serde_json::to_value(&net).expect("serialize network with uids");
    ensure_payload_uids(&mut net);
    let second = serde_json::to_value(&net).expect("serialize network with uids again");

    assert_eq!(first, second);
    assert_eq!(net.buses[0].uid.as_deref(), Some("source-bus"));
    assert_eq!(net.buses[1].uid.as_deref(), Some("buses:1"));
    assert_eq!(net.loads[0].uid.as_deref(), Some("source-load"));
    assert_eq!(net.generators[0].uid.as_deref(), Some("generators:0"));
}

#[test]
fn study_commit_materialization_folds_commits_and_set_fields() {
    let study = study_block(vec![
        study_commit(vec![
            StudyEdit::DemandDelta {
                bus: ElementRef::by_source_uid("buses", "buses:1"),
                p_mw: 5.0,
                q_mvar: None,
            },
            StudyEdit::RatingDelta {
                branch: ElementRef::by_source_uid("branches", "branch_1"),
                delta_mw: -10.0,
            },
        ]),
        study_commit(vec![
            StudyEdit::DemandDelta {
                bus: ElementRef::by_source_uid("buses", "buses:1"),
                p_mw: 5.0,
                q_mvar: Some(1.0),
            },
            StudyEdit::SetFields {
                update: ElementUpdate::new(
                    ElementRef::by_source_uid("generators", "gen_1"),
                    fields(&[("pg", serde_json::json!(55.0))]),
                ),
            },
        ]),
    ]);
    let pkg = balanced_package_with_gen()
        .with_package_id("parent")
        .with_study(study);

    let materialized = pkg.materialize_study_commit(1).expect("materialize study");
    assert!(materialized.study.is_none());
    assert!(materialized.operating_points.is_none());
    assert!(matches!(materialized.origin, Origin::Derived { .. }));
    assert!(
        materialized
            .lowering_history
            .iter()
            .any(|record| record.pass == "materialize-study-commit")
    );

    let net = materialized.as_balanced().expect("balanced output");
    assert_close(net.loads[0].p, 20.0);
    assert_close(net.loads[0].q, 8.5);
    assert_close(net.branches[0].rate_a, 90.0);
    assert_close(net.generators[0].pg, 55.0);
}

#[test]
fn study_demand_delta_distributes_over_existing_loads() {
    let mut net = powerio::parse_str(MATPOWER_WITH_GEN_SRC, "matpower")
        .expect("parse matpower")
        .network;
    net.loads[0].uid = Some("load_1".to_owned());
    let mut extra_load = powerio::Load::new(powerio::BusId(2), 30.0, 15.0);
    extra_load.uid = Some("load_2".to_owned());
    net.loads.push(extra_load);

    let study = study_block(vec![study_commit(vec![StudyEdit::DemandDelta {
        bus: ElementRef::by_source_uid("buses", "buses:1"),
        p_mw: 8.0,
        q_mvar: None,
    }])]);
    let materialized = NetworkPackage::from_balanced(net)
        .with_study(study)
        .materialize_study_commit(0)
        .expect("materialize proportional demand delta");
    let net = materialized.as_balanced().expect("balanced output");

    assert_close(net.loads[0].p, 12.0);
    assert_close(net.loads[0].q, 6.0);
    assert_close(net.loads[1].p, 36.0);
    assert_close(net.loads[1].q, 18.0);
}

#[test]
fn study_demand_delta_appends_synthetic_load_for_empty_bus() {
    let mut net = powerio::parse_str(MATPOWER_SRC, "matpower")
        .expect("parse matpower")
        .network;
    net.loads.clear();
    let study = study_block(vec![study_commit(vec![StudyEdit::DemandDelta {
        bus: ElementRef::by_source_uid("buses", "buses:1"),
        p_mw: 12.0,
        q_mvar: Some(3.0),
    }])]);

    let materialized = NetworkPackage::from_balanced(net)
        .with_study(study)
        .materialize_study_commit(0)
        .expect("materialize synthetic load");
    let net = materialized.as_balanced().expect("balanced output");

    assert_eq!(net.loads.len(), 1);
    assert_eq!(net.loads[0].bus, powerio::BusId(2));
    assert_close(net.loads[0].p, 12.0);
    assert_close(net.loads[0].q, 3.0);
    assert_eq!(net.loads[0].uid.as_deref(), Some("study:load:buses:1"));
    assert_eq!(
        net.loads[0]
            .extras
            .get("study")
            .and_then(|v| v.get("synthetic"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[test]
fn study_uses_base_operating_point_before_commits() {
    let mut study = study_block(vec![study_commit(vec![StudyEdit::DemandDelta {
        bus: ElementRef::by_source_uid("buses", "buses:1"),
        p_mw: 2.0,
        q_mvar: Some(1.0),
    }])]);
    study.base_operating_point = Some(1);

    let materialized = balanced_package_with_gen()
        .with_operating_points(sample_operating_points())
        .with_study(study)
        .materialize_study_commit(0)
        .expect("materialize study from operating point");
    let net = materialized.as_balanced().expect("balanced output");

    assert_close(net.loads[0].p, 24.0);
    assert_close(net.loads[0].q, 10.0);
    assert!(materialized.operating_points.is_none());
    assert!(materialized.study.is_none());
    assert_eq!(
        materialized
            .lowering_history
            .iter()
            .map(|record| record.pass.as_str())
            .collect::<Vec<_>>(),
        vec!["materialize-operating-point", "materialize-study-commit"]
    );
}

#[test]
fn study_unknown_edit_kind_roundtrips_and_refuses_materialization() {
    let mut value = serde_json::to_value(balanced_package()).expect("serialize package");
    value["study"] = serde_json::json!({
        "commits": [
            {
                "edits": [
                    {
                        "kind": "solver_knob",
                        "value": {"name": "shed", "enabled": false},
                        "extra": [1, 2, 3]
                    }
                ]
            }
        ]
    });

    let pkg: NetworkPackage = serde_json::from_value(value.clone()).expect("parse package");
    let round_trip = serde_json::to_value(&pkg).expect("serialize package again");
    assert_eq!(round_trip["study"], value["study"]);

    let err = pkg
        .materialize_study_commit(0)
        .expect_err("unknown study edit kind must fail");
    assert!(err.to_string().contains("STUDY.UNKNOWN_EDIT_KIND"), "{err}");
    assert!(err.to_string().contains("solver_knob"), "{err}");
}

#[test]
fn study_set_fields_rejects_fields_not_in_typed_model() {
    let study = study_block(vec![study_commit(vec![StudyEdit::SetFields {
        update: ElementUpdate::new(
            ElementRef::by_source_uid("loads", "load_1"),
            fields(&[("ghost_field", serde_json::json!(1.0))]),
        ),
    }])]);

    let err = balanced_package_with_gen()
        .with_study(study)
        .materialize_study_commit(0)
        .expect_err("unknown field should not survive typed materialization");
    assert!(err.to_string().contains("field `ghost_field`"), "{err}");
}

#[test]
fn study_validation_reports_bad_identity() {
    let study = study_block(vec![study_commit(vec![StudyEdit::DemandDelta {
        bus: ElementRef::by_source_uid("buses", "ghost"),
        p_mw: 1.0,
        q_mvar: None,
    }])]);
    let mut pkg = balanced_package().with_study(study);
    pkg.run_sane_validation();

    assert!(pkg.diagnostics.iter().any(|d| {
        d.code.as_str() == "VALIDATE.PACKAGE.STUDY_IDENTITY"
            && d.element_path.as_deref() == Some("/study/commits/0/edits/0")
    }));
    assert_eq!(pkg.validation.status, ValidationStatus::Error);
}

#[test]
fn study_validation_rejects_multiconductor_payload() {
    let study = study_block(vec![study_commit(Vec::new())]);
    let mut pkg = multiconductor_package().with_study(study);
    pkg.run_sane_validation();

    assert!(pkg.diagnostics.iter().any(|d| {
        d.code.as_str() == "VALIDATE.PACKAGE.STUDY_MODEL_KIND"
            && d.element_path.as_deref() == Some("/study")
    }));
    assert_eq!(pkg.validation.status, ValidationStatus::Error);
}

/// BMOPF schema 0.1.0 gives a line its own `i_max`, which "overrides the
/// linecode's i_max for this line". The lowering must take the line field
/// where it is present, or the balanced branch rating is the rating of a
/// shared linecode instead of the rating of the line.
#[test]
fn a_line_rating_overrides_the_linecode_rating_in_the_lowering() {
    let source = "\
New Circuit.c basekv=4.16 bus1=b1\n\
New Linecode.lc nphases=3 rmatrix=[1|0 1|0 0 1] xmatrix=[1|0 1|0 0 1] normamps=600 emergamps=600\n\
New Line.l1 bus1=b1.1.2.3 bus2=b2.1.2.3 linecode=lc length=1 units=m\n";
    let net = powerio_dist::parse_str(source, "dss").expect("parse dss");
    let shared = lower_multiconductor_to_balanced(&net, MulticonductorToBalancedOptions::default())
        .expect("lower")
        .network;
    let shared_rate = shared.branches[0].rate_a;

    let mut with_line_rating = net.clone();
    with_line_rating.lines[0].i_max = Some(vec![200.0, 200.0, 200.0]);
    let lowered = lower_multiconductor_to_balanced(
        &with_line_rating,
        MulticonductorToBalancedOptions::default(),
    )
    .expect("lower")
    .network;
    let line_rate = lowered.branches[0].rate_a;

    assert!(shared_rate > 0.0, "the linecode rating is the baseline");
    assert!(
        line_rate < shared_rate,
        "the line's 200 A must beat the linecode's 600 A: {line_rate} vs {shared_rate}"
    );
}

/// A rated capacitor bank has no balanced equivalent yet, so it drops. The
/// lowering record must name the drop, and the multiconductor summary must
/// count the bank, or the package under-reports the case.
#[test]
fn a_dropped_capacitor_bank_is_recorded_and_counted() {
    let mut net = powerio_dist::parse_str("New Circuit.c1", "dss").expect("parse dss");
    net.capacitors.push(powerio_dist::DistCapacitor::new(
        "cap1",
        "sourcebus",
        vec!["1".to_owned()],
        powerio_dist::Configuration::Wye,
        300_000.0,
        7200.0,
    ));

    let package = NetworkPackage::from_multiconductor(net.clone());
    assert_eq!(package.summary.elements["capacitors"], 1);

    let lowering =
        lower_multiconductor_to_balanced(&net, MulticonductorToBalancedOptions::default())
            .expect("lower");
    assert!(
        lowering
            .record
            .dropped_fields
            .iter()
            .any(|f| f.contains("capacitor cap1")),
        "{:?}",
        lowering.record.dropped_fields
    );
}

#[test]
fn multiconductor_nonfinite_floats_roundtrip() {
    // #268: serde_json writes a nonfinite f64 as `null`. The payload reader
    // restores the bound's infinity and a length NaN, so a package the
    // library wrote always reads back.
    use powerio_dist::{Configuration, DistGenerator, DistSwitch};

    let mut net = powerio_dist::parse_str("New Circuit.c1", "dss").expect("parse dss");
    let mut generator = DistGenerator::new(
        "g1",
        "sourcebus",
        vec!["1".into(), "2".into()],
        Configuration::Wye,
        vec![100e3, 100e3],
        vec![0.0, 0.0],
    );
    generator.p_max = Some(vec![500e3, f64::INFINITY]);
    generator.p_min = Some(vec![f64::NEG_INFINITY, 0.0]);
    net.generators.push(generator);
    let mut switch = DistSwitch::new(
        "s1",
        "sourcebus",
        "sourcebus",
        vec!["1".into()],
        vec!["1".into()],
        false,
    );
    switch.i_max = Some(vec![f64::INFINITY]);
    net.switches.push(switch);

    let pkg = NetworkPackage::from_multiconductor(net);
    let text = pkg.to_json().expect("serialize");
    let back = NetworkPackage::from_json(&text).expect("read back the package this wrote");

    let payload = back.as_multiconductor().expect("multiconductor payload");
    let generator = &payload.generators[payload.generators.len() - 1];
    assert_eq!(generator.p_max, Some(vec![500e3, f64::INFINITY]));
    assert_eq!(generator.p_min, Some(vec![f64::NEG_INFINITY, 0.0]));
    let switch = &payload.switches[payload.switches.len() - 1];
    assert_eq!(switch.i_max, Some(vec![f64::INFINITY]));
}

#[test]
fn multiconductor_nonfinite_ratings_and_scalars_roundtrip() {
    // #268: an inverter bound, a capacitor rating, and a transformer
    // winding rating take the same `null` spelling as the generator bounds.
    use powerio_dist::{Configuration, DistCapacitor, DistIbr, IbrPrimeMover, IbrTopology};

    let mut net = powerio_dist::parse_str("New Circuit.c1", "dss").expect("parse dss");
    let mut ibr = DistIbr::new(
        "i1",
        "sourcebus",
        vec!["1".into()],
        IbrTopology::ThreeLeg,
        IbrPrimeMover::Pv,
        vec![f64::INFINITY],
    );
    ibr.p_max = Some(vec![f64::INFINITY]);
    ibr.p_min = Some(vec![f64::NEG_INFINITY]);
    ibr.i_max = Some(vec![f64::INFINITY]);
    net.ibrs.push(ibr);
    net.capacitors.push(DistCapacitor::new(
        "c1",
        "sourcebus",
        vec!["1".into()],
        Configuration::Wye,
        f64::NAN,
        f64::NAN,
    ));

    let pkg = NetworkPackage::from_multiconductor(net);
    let text = pkg.to_json().expect("serialize");
    let back = NetworkPackage::from_json(&text).expect("read back the package this wrote");

    let payload = back.as_multiconductor().expect("multiconductor payload");
    let ibr = &payload.ibrs[payload.ibrs.len() - 1];
    assert_eq!(ibr.s_max, vec![f64::INFINITY]);
    assert_eq!(ibr.p_max, Some(vec![f64::INFINITY]));
    assert_eq!(ibr.p_min, Some(vec![f64::NEG_INFINITY]));
    assert_eq!(ibr.i_max, Some(vec![f64::INFINITY]));
    let capacitor = &payload.capacitors[payload.capacitors.len() - 1];
    assert!(capacitor.q_rated.is_nan());
    assert!(capacitor.v_nom.is_nan());
}

#[test]
fn refused_include_lifts_as_an_error_diagnostic() {
    // #275: a typed parse finding keeps its severity in the document, and
    // its warning twin does not appear a second time.
    use powerio_pkg::{DiagnosticSeverity, ValidationStatus};

    let mut net = powerio_dist::parse_str("New Circuit.c1", "dss").expect("parse dss");
    let message = "redirect ../shared.dss: refused; include escapes the case directory";
    net.warnings.push(message.to_owned());
    net.parse_diagnostics
        .push(powerio_dist::StructuredDiagnostic::new(
            powerio_dist::diagnostics::READ_DSS_INCLUDE_REFUSED,
            powerio_dist::DiagnosticSeverity::Error,
            powerio_dist::DiagnosticStage::Parse,
            message,
        ));

    let pkg = NetworkPackage::from_multiconductor(net);
    let carrying: Vec<_> = pkg
        .diagnostics
        .iter()
        .filter(|d| d.message == message)
        .collect();
    assert_eq!(carrying.len(), 1, "the finding must appear exactly once");
    assert_eq!(carrying[0].severity, DiagnosticSeverity::Error);
    assert_eq!(pkg.validation.status, ValidationStatus::Error);
}

use super::*;
use crate::dss::raw::__parse_raw_with;

fn read(text: &str) -> RawDss {
    __parse_raw_with(text, "linecode.dss", &mut |_: &std::path::Path| {
        Err(std::io::Error::other("no includes"))
    })
}

fn point(raw: &RawDss, name: &str) -> Snapshot {
    raw.line_snapshot(
        raw.objects
            .iter()
            .position(|o| o.class == "linecode" && o.name == name)
            .unwrap(),
    )
}

#[test]
fn matrix_copy_preserves_destination_mode_and_units() {
    let raw = read(
        "New Circuit.c\nNew Linecode.a nphases=1 rmatrix=[1] xmatrix=[2] cmatrix=[3] units=km\nNew Linecode.b like=a\nNew Linecode.c rmatrix=[9|0 9|0 0 9] units=ft like=a",
    );
    let b = resolve(&raw, point(&raw, "b")).unwrap();
    assert!(b.sequence);
    assert_eq!(b.units, 0);
    assert!((b.r[0][0] - 0.098_133_333_333_333_32).abs() < 1e-15);
    let c = resolve(&raw, point(&raw, "c")).unwrap();
    assert!(!c.sequence);
    assert_eq!(c.units, 5);
    assert_eq!(c.r, vec![vec![1.0]]);
}

#[test]
fn matrix_charging_keeps_assignment_frequency() {
    let raw = read("New Circuit.c\nNew Linecode.a nphases=1 basefreq=50 cmatrix=[12] basefreq=60");
    let state = resolve(&raw, point(&raw, "a")).unwrap();
    assert_eq!(state.frequency.to_bits(), 60.0_f64.to_bits());
    assert!((state.b[0][0] - TAU * 50.0 * 12e-9).abs() < 1e-18);
}

#[test]
fn copied_checkpoints_are_not_destination_edits() {
    // a is in matrix mode but has updated sequence scalars. Copying into b
    // must not close b's edit before its final matrix setter: X stays 2.
    let raw = read(
        "New Circuit.c\nNew Linecode.a nphases=1 r1=0.7\nEdit Linecode.a rmatrix=[1] xmatrix=[2]\nNew Linecode.b like=a rmatrix=[3]",
    );
    let b = resolve(&raw, point(&raw, "b")).unwrap();
    assert_eq!(b.r, vec![vec![3.0]]);
    assert_eq!(b.x, vec![vec![2.0]]);
}

#[test]
fn historical_line_binding_does_not_follow_late_code_edits() {
    let raw = read(
        "New Circuit.c\nNew Linecode.a nphases=1 rmatrix=[1]\nNew Line.first linecode=a\nEdit Linecode.a rmatrix=[2]\nNew Line.second linecode=a",
    );
    let states: Vec<_> = raw
        .objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.class == "line")
        .map(|(i, _)| {
            let snapshot = raw.line_history[&i]
                .linecodes
                .values()
                .next()
                .unwrap()
                .unwrap();
            resolve(&raw, snapshot).unwrap().r[0][0]
        })
        .collect();
    assert_eq!(states, vec![1.0, 2.0]);
}

#[test]
fn matrix_c0_setter_does_not_switch_to_sequence_mode() {
    let raw = read("New Circuit.c\nNew Linecode.a nphases=1 rmatrix=[1] c0=9");
    let state = resolve(&raw, point(&raw, "a")).unwrap();
    assert!(!state.sequence);
    assert_eq!(state.r, vec![vec![1.0]]);
}

#[test]
fn empty_nested_and_self_copies_preserve_edit_order() {
    let raw = read(
        "New Circuit.c\nNew Linecode.a\nEdit Linecode.a like=a\nNew Linecode.b like=a\nEdit Linecode.b like=b basefreq=50\nNew Linecode.c like=b",
    );
    let state = resolve(&raw, point(&raw, "c")).unwrap();
    assert_eq!(state.frequency.to_bits(), 50.0_f64.to_bits());
    assert!((state.b[0][0] - TAU * 50.0 * 2.8e-9).abs() < 1e-18);
}

#[test]
fn long_copy_chain_resolves_electrical_state_without_recursion() {
    use std::fmt::Write as _;
    let mut text = "New Circuit.c\nNew Linecode.c0\n".to_owned();
    for i in 1..=512 {
        writeln!(text, "New Linecode.c{i} like=c{}", i - 1).unwrap();
    }
    let raw = read(&text);
    let state = resolve(&raw, point(&raw, "c512")).unwrap();
    assert!((state.r[0][0] - 0.098_133_333_333_333_32).abs() < 1e-15);
}

#[derive(serde::Deserialize)]
struct Native {
    n: usize,
    frequency: f64,
    units: usize,
    sequence: bool,
    r: Vec<f64>,
    x: Vec<f64>,
    b: Vec<f64>,
    rg: f64,
    xg: f64,
    rho: f64,
    neutral: usize,
    normamps: f64,
    emergamps: f64,
}

#[derive(serde::Deserialize)]
struct NativeLine {
    operating: f64,
    length: f64,
    units: usize,
    r: Vec<f64>,
    x: Vec<f64>,
    b: Vec<f64>,
}

fn matrices_match(label: &str, actual: [&Vec<Vec<f64>>; 3], expected: [&Vec<f64>; 3]) -> usize {
    let mut compared = 0;
    for (a, b) in actual.into_iter().zip(expected) {
        assert_eq!(a.len() * a.len(), b.len(), "{label}: matrix dimension");
        assert!(a.iter().all(|row| row.len() == a.len()));
        for (&a, &b) in a.iter().flatten().zip(b) {
            assert!(
                (a - b).abs() <= 1e-13 + 1e-11 * b.abs(),
                "{label}: {a} != {b}"
            );
            compared += 1;
        }
    }
    compared
}

fn code_matches(label: &str, state: &State, expected: &Native) -> usize {
    assert_eq!(
        (state.n, state.units, state.sequence, state.neutral),
        (
            expected.n,
            expected.units,
            expected.sequence,
            expected.neutral
        ),
        "{label}"
    );
    let actual = [
        state.frequency,
        state.rg,
        state.xg,
        state.rho,
        state.normamps,
        state.emergamps,
    ];
    let reference = [
        expected.frequency,
        expected.rg,
        expected.xg,
        expected.rho,
        expected.normamps,
        expected.emergamps,
    ];
    for (a, b) in actual.into_iter().zip(reference) {
        assert!(
            (a - b).abs() <= 1e-13 + 1e-11 * b.abs(),
            "{label}: {a} != {b}"
        );
    }
    6 + matrices_match(
        label,
        [&state.r, &state.x, &state.b],
        [&expected.r, &expected.x, &expected.b],
    )
}

#[test]
fn direct_linecode_uses_frequency_correction_and_single_phase_sequence() {
    let raw = read(
        "New Circuit.c\nNew Linecode.a nphases=1 r1=0.2 x1=0.4 r0=0.8 x0=1.2 c1=12 c0=6 basefreq=50 units=km",
    );
    let state = resolve(&raw, point(&raw, "a")).unwrap();
    let line = state.line_matrices(60.0, 500.0, 4).unwrap();
    assert!((line.r[0][0] - (0.2 + 0.01805 * 0.2) * 0.5).abs() < 1e-15);
    assert!((line.b[0][0] - TAU * 60.0 * 12e-9 * 0.5).abs() < 1e-18);
    assert!(state.line_matrices(0.0, 1.0, 0).is_err());
}

#[test]
#[ignore = "requires pinned native linecode matrix observations"]
fn pinned_linecode_matrices() {
    #[derive(serde::Deserialize)]
    struct Case {
        script: String,
        codes: std::collections::BTreeMap<String, Native>,
        lines: std::collections::BTreeMap<String, NativeLine>,
    }
    #[derive(serde::Deserialize)]
    struct Report {
        cases: std::collections::BTreeMap<String, Case>,
        failures: Vec<String>,
    }
    let path = std::env::var("POWERIO_LINECODE_ORACLE").expect("native observations");
    let report: Report = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(report.failures.is_empty());
    assert!(!report.cases.is_empty());
    let mut findings = std::collections::BTreeMap::new();
    let mut matrices = std::collections::BTreeMap::new();
    for (name, case) in report.cases {
        let raw = read(&case.script);
        let mut compared = 0;
        let mut case_matrices = std::collections::BTreeMap::new();
        assert_eq!(case.codes.len(), case.lines.len());
        for (code, expected) in case.codes {
            let label = format!("{name}/{code}");
            let state =
                resolve(&raw, point(&raw, &code)).unwrap_or_else(|e| panic!("{label}: {e}"));
            compared += code_matches(&label, &state, &expected);
            let expected = &case.lines[&code];
            let actual = state
                .line_matrices(expected.operating, expected.length, expected.units)
                .unwrap();
            compared += matrices_match(
                &format!("{label}/line"),
                [&actual.r, &actual.x, &actual.b],
                [&expected.r, &expected.x, &expected.b],
            );
            case_matrices.insert(
                code,
                serde_json::json!({"r": actual.r, "x": actual.x, "b": actual.b}),
            );
        }
        assert!(compared > 0);
        matrices.insert(name.clone(), case_matrices);
        findings.insert(name, compared);
    }
    let output = std::env::var("POWERIO_LINECODE_MATRICES").expect("matrix export path");
    std::fs::write(output, serde_json::to_vec_pretty(&matrices).unwrap()).unwrap();
    let output = std::env::var("POWERIO_LINECODE_RESULT").expect("comparison path");
    std::fs::write(output, serde_json::to_vec_pretty(&findings).unwrap()).unwrap();
}

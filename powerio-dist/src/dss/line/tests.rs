use super::*;
use crate::dss::raw::__parse_raw_with;

fn read(script: &str) -> RawDss {
    __parse_raw_with(script, "line.dss", &mut |_: &std::path::Path| {
        Err(std::io::Error::other("no includes"))
    })
}

fn point(raw: &RawDss, name: &str) -> Snapshot {
    raw.line_snapshot(
        raw.objects
            .iter()
            .position(|o| o.class == "line" && o.name == name)
            .unwrap(),
    )
}

#[test]
fn copy_before_solve_keeps_the_sources_stored_matrix() {
    let raw = read("New Circuit.c\nNew Line.a r1=0.7\nNew Line.b like=a\nSolve\nNew Line.c like=a");
    let mut b = resolve(&raw, point(&raw, "b")).unwrap();
    let mut c = resolve(&raw, point(&raw, "c")).unwrap();
    let b = b.primitive(60.0).unwrap();
    let c = c.primitive(60.0).unwrap();
    assert!((b.r[0][0] - 0.098_133_333_333_333_32).abs() < 1e-15);
    assert!((c.r[0][0] - (2.0 * 0.7 + 0.1784) / 3.0).abs() < 1e-15);
}

#[test]
fn copied_line_leaves_destination_buses_units_and_switch_flag() {
    let raw = read(
        "New Circuit.c\nNew Line.a bus1=first bus2=second switch=yes\nNew Line.b bus1=feed bus2=end units=km like=a",
    );
    let b = resolve(&raw, point(&raw, "b")).unwrap();
    assert_eq!(b.bus, ["feed", "end"]);
    assert_eq!(b.units, 3);
    assert!(!b.is_switch);
}

#[test]
fn solved_base_frequency_edits_need_primitive_invalidation() {
    let raw = read(
        "New Circuit.c\nNew Line.a\nSolve\nEdit Line.a basefreq=45\nSolve\nNew Line.b like=a\nBuildY\nEdit Line.a length=1\nSolve",
    );
    let mut a = resolve(&raw, point(&raw, "a")).unwrap();
    let a = a.primitive(60.0).unwrap();
    assert!((a.r[0][0] - (0.098_133_333_333_333_32 + 0.01805 / 3.0)).abs() < 1e-15);
}

#[test]
fn singular_lines_are_not_replaced_by_epsilon_conductance() {
    let raw = read("New Circuit.c\nNew Line.a phases=1 r1=0 x1=0 rg=0 xg=0\nSolve");
    assert!(
        resolve(&raw, point(&raw, "a"))
            .unwrap_err()
            .contains("error 183")
    );
    let raw = read("New Circuit.c\nNew Line.a phases=2 rmatrix=[1|0 1] phases=3");
    assert!(
        resolve(&raw, point(&raw, "a"))
            .unwrap_err()
            .contains("error 18101")
    );
}

#[test]
fn buildy_does_not_invalidate_an_existing_line_primitive() {
    let raw = read("New Circuit.c\nNew Line.a\nSolve\nEdit Line.a basefreq=45\nBuildY\nSolve");
    let matrix = resolve(&raw, point(&raw, "a"))
        .unwrap()
        .primitive(60.0)
        .unwrap();
    assert!((matrix.r[0][0] - 0.098_133_333_333_333_32).abs() < 1e-15);
}

#[test]
fn line_copies_do_not_recurse_through_long_empty_chains() {
    use std::fmt::Write as _;
    let mut script = "New Circuit.c\nNew Line.l0\n".to_owned();
    for i in 1..=512 {
        writeln!(script, "New Line.l{i} like=l{}", i - 1).unwrap();
    }
    let raw = read(&script);
    let matrix = resolve(&raw, point(&raw, "l512"))
        .unwrap()
        .primitive(60.0)
        .unwrap();
    assert!((matrix.r[0][0] - 0.098_133_333_333_333_32).abs() < 1e-15);
}

#[test]
fn linecode_work_shares_the_line_resolution_budget() {
    let raw = read("New Circuit.c\nNew Linecode.a r1=0.7\nNew Line.b linecode=a");
    let code = raw.line_history[&point(&raw, "b").object]
        .linecodes
        .values()
        .next()
        .unwrap()
        .unwrap();
    assert!(
        linecode::resolve_with_budget(&raw, code, &mut 1)
            .unwrap_err()
            .contains("work limit")
    );
}

#[test]
#[ignore = "requires pinned native ordered Line observations"]
fn pinned_line_states() {
    #[derive(serde::Deserialize)]
    struct Native {
        frequency: f64,
        n: usize,
        units: usize,
        bus: Vec<String>,
        enabled: bool,
        is_switch: bool,
        normamps: f64,
        emergamps: f64,
        length: f64,
    }
    #[derive(serde::Deserialize)]
    struct Case {
        script: String,
        lines: std::collections::BTreeMap<String, Native>,
    }
    #[derive(serde::Deserialize)]
    struct Report {
        cases: std::collections::BTreeMap<String, Case>,
        failures: Vec<String>,
    }
    let report: Report = serde_json::from_slice(
        &std::fs::read(std::env::var("POWERIO_LINE_ORACLE").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(report.failures, [] as [std::string::String; 0]);
    assert!(!report.cases.is_empty());
    let mut output = std::collections::BTreeMap::new();
    for (name, case) in report.cases {
        let raw = read(&case.script);
        let mut results = std::collections::BTreeMap::new();
        for (line, expected) in case.lines {
            let mut state =
                resolve(&raw, point(&raw, &line)).unwrap_or_else(|e| panic!("{name}/{line}: {e}"));
            assert_eq!(
                (state.n, state.units, state.enabled, state.is_switch),
                (
                    expected.n,
                    expected.units,
                    expected.enabled,
                    expected.is_switch
                ),
                "{name}/{line}"
            );
            assert_eq!(state.bus.as_slice(), expected.bus, "{name}/{line}");
            for (a, b) in [
                state.frequency,
                state.normamps,
                state.emergamps,
                state.length,
            ]
            .into_iter()
            .zip([
                expected.frequency,
                expected.normamps,
                expected.emergamps,
                expected.length,
            ]) {
                assert!(
                    (a - b).abs() <= 1e-12 + 1e-12 * b.abs(),
                    "{name}/{line}: {a} != {b}"
                );
            }
            if state.enabled {
                let matrices = state
                    .primitive(raw.snapshot_frequency())
                    .unwrap_or_else(|e| panic!("{name}/{line}: {e}"));
                results.insert(
                    line,
                    serde_json::json!({"r":matrices.r,"x":matrices.x,"b":matrices.b}),
                );
            }
        }
        output.insert(name, results);
    }
    std::fs::write(
        std::env::var("POWERIO_LINE_MATRICES").unwrap(),
        serde_json::to_vec_pretty(&output).unwrap(),
    )
    .unwrap();
}

#[test]
fn fixed_terminal_history_follows_native_copy_and_resize_rules() {
    for (commands, expected) in [
        ("open line.a 1 2", [vec![true, false, true], vec![true; 3]]),
        (
            "open line.a 1\nclose line.a 1 2",
            [vec![false, true, false], vec![true; 3]],
        ),
        (
            "open line.a 2 1\nedit line.a phases=3",
            [vec![true; 3], vec![false, true, true]],
        ),
        (
            "open line.a 2 1\nedit line.a phases=2\nedit line.a phases=3",
            [vec![true; 3], vec![true; 3]],
        ),
        (
            "open line.a 2 1\nnew line.b\nedit line.a like=b",
            [vec![true; 3], vec![false, true, true]],
        ),
        (
            "new line.b\nopen line.b 1\nedit line.a like=b",
            [vec![true; 3], vec![true; 3]],
        ),
        (
            "open line.a 2\nclose line.a term=2 cond=0",
            [vec![true; 3], vec![true; 3]],
        ),
    ] {
        let raw = read(&format!("new circuit.c\nnew line.a\n{commands}\nsolve"));
        assert_eq!(
            resolve(&raw, point(&raw, "a")).unwrap().closed,
            expected,
            "{commands}"
        );
    }
}

#[test]
fn terminal_command_binds_before_later_declarations_and_selects_the_line() {
    let raw = read("new circuit.c\nopen line.a 1\nnew line.a");
    assert!(
        raw.warnings
            .iter()
            .any(|w| w.contains("requires an existing Line or Transformer"))
    );
    let raw = read("new circuit.c\nnew line.a\nnew line.b\nopen line.a 1 2\n~ length=7");
    assert_eq!(
        resolve(&raw, point(&raw, "a")).unwrap().length.to_bits(),
        7.0_f64.to_bits()
    );
    assert_eq!(
        resolve(&raw, point(&raw, "b")).unwrap().length.to_bits(),
        1.0_f64.to_bits()
    );
}

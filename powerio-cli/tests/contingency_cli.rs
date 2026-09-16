//! `powerio contingency` binds the three PSS/E contingency analysis files to
//! a case.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_powerio")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/psse/contingency")
        .join(name)
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn stdout_of(out: &Output) -> String {
    assert!(
        out.status.success(),
        "exit {:?}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn resolve_counts_the_cases_and_names_each_one_that_bound_to_nothing() {
    let case = fixture("resolve_v33.raw");
    let con = fixture("resolve_cases.con");
    let out = run(&[
        "contingency",
        "resolve",
        case.to_str().unwrap(),
        con.to_str().unwrap(),
    ]);
    let text = stdout_of(&out);
    assert!(
        text.contains("cases 20: 17 resolved, 3 unresolved"),
        "{text}"
    );
    assert!(
        text.contains("unresolved BR_MISSING: names no branch"),
        "{text}"
    );
    assert!(
        text.contains("unresolved MACHINE_MISSING: names no machine"),
        "{text}"
    );
    assert!(
        text.contains("unresolved UNRECOGNIZED: was kept as text and names no element"),
        "{text}"
    );
}

#[test]
fn resolve_reports_the_same_counts_as_one_json_object() {
    let case = fixture("resolve_v33.raw");
    let con = fixture("resolve_cases.con");
    let out = run(&[
        "contingency",
        "resolve",
        case.to_str().unwrap(),
        con.to_str().unwrap(),
        "--json",
    ]);
    let report: serde_json::Value = serde_json::from_str(&stdout_of(&out)).unwrap();
    assert_eq!(report["schema"], "powerio.contingency_resolution");
    assert_eq!(report["cases"], 20);
    assert_eq!(report["resolved"], 17);
    assert_eq!(report["unresolved"], 3);
    assert_eq!(report["unrecognized_statements"], 1);
    let names: Vec<&str> = report["unresolved_cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["case"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["BR_MISSING", "MACHINE_MISSING", "UNRECOGNIZED"]);
    assert!(report.get("monitored").is_none(), "{report}");
}

#[test]
fn a_monitored_element_file_adds_its_own_counts() {
    let case = fixture("select_v33.raw");
    let con = fixture("expand.con");
    let sub = fixture("selectors.sub");
    let mon = fixture("blocks.mon");
    let out = run(&[
        "contingency",
        "resolve",
        case.to_str().unwrap(),
        con.to_str().unwrap(),
        "--sub",
        sub.to_str().unwrap(),
        "--mon",
        mon.to_str().unwrap(),
        "--json",
    ]);
    let report: serde_json::Value = serde_json::from_str(&stdout_of(&out)).unwrap();
    assert_eq!(report["monitored"]["branches"], 3);
    assert_eq!(report["monitored"]["interfaces"], 2);
    assert_eq!(report["monitored"]["unresolved"], 2);
}

#[test]
fn a_subsystem_file_is_read_with_a_monitored_element_file() {
    let case = fixture("select_v33.raw");
    let con = fixture("expand.con");
    let sub = fixture("selectors.sub");
    let out = run(&[
        "contingency",
        "resolve",
        case.to_str().unwrap(),
        con.to_str().unwrap(),
        "--sub",
        sub.to_str().unwrap(),
    ]);
    // The subsystems name the buses a monitored statement works over, so
    // --sub alone states a file nothing would read.
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--mon"), "{stderr}");
}

#[test]
fn expand_writes_the_generated_cases_as_con_text() {
    let case = fixture("select_v33.raw");
    let con = fixture("expand.con");
    let sub = fixture("selectors.sub");
    let out = run(&[
        "contingency",
        "expand",
        case.to_str().unwrap(),
        con.to_str().unwrap(),
        "--sub",
        sub.to_str().unwrap(),
    ]);
    let text = stdout_of(&out);
    for name in [
        "CONTINGENCY 'EXPLICIT'",
        "CONTINGENCY 'L_101_102_1'",
        "CONTINGENCY 'L_102_103_1'",
        "CONTINGENCY 'T_101_102_103_1'",
        "CONTINGENCY 'G_101_1'",
        "CONTINGENCY 'L_103_201_1'",
        "CONTINGENCY 'L_101_102_1+L_102_103_1'",
    ] {
        assert!(text.contains(name), "{name} is absent from:\n{text}");
    }
    // The specification naming a subsystem the set does not state keeps its
    // SKIP rules and stays unexpanded.
    assert!(
        text.contains("SINGLE BRANCH IN SUBSYSTEM 'NOSUCH'"),
        "{text}"
    );
    assert!(text.contains("SKIP"), "{text}");
    // A branch the SKIP rule names never becomes a case.
    assert!(!text.contains("CONTINGENCY 'L_101_102_2'"), "{text}");
}

#[test]
fn expand_writes_to_a_named_file() {
    let out_dir = std::env::temp_dir().join(format!("powerio-contingency-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let target = out_dir.join("expanded.con");
    let case = fixture("select_v33.raw");
    let con = fixture("expand.con");
    let sub = fixture("selectors.sub");
    let out = run(&[
        "contingency",
        "expand",
        case.to_str().unwrap(),
        con.to_str().unwrap(),
        "--sub",
        sub.to_str().unwrap(),
        "-o",
        target.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.contains("CONTINGENCY 'L_101_102_1'"), "{text}");
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// The `.con` reader keeps a statement outside its grammar as text rather
/// than failing, so a file of foreign statements reads as a set of no cases
/// and every line is reported.
#[test]
fn a_file_of_foreign_statements_reads_as_no_cases() {
    let case = fixture("resolve_v33.raw");
    let out = run(&[
        "contingency",
        "resolve",
        case.to_str().unwrap(),
        case.to_str().unwrap(),
        "--json",
    ]);
    let report: serde_json::Value = serde_json::from_str(&stdout_of(&out)).unwrap();
    assert_eq!(report["cases"], 0);
    assert_eq!(report["unresolved"], 0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("READ.CON.STATEMENT_UNRECOGNIZED"),
        "{stderr}"
    );
}

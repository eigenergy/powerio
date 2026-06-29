use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_powerio")
}

fn repo_file(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(path)
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "expected success\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_failure(out: &Output) {
    assert!(
        !out.status.success(),
        "expected failure\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn help_lists_user_facing_commands() {
    let out = run(&["--help"]);
    assert_success(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Convert a case file"), "{stdout}");
    assert!(stdout.contains("summary"), "{stdout}");
    assert!(stdout.contains("gridfm"), "{stdout}");
}

#[test]
fn convert_to_stdout_keeps_text_on_stdout() {
    let case = repo_file("tests/data/case9.m");
    let out = run(&[
        "convert",
        case.to_str().unwrap(),
        "--to",
        "matpower",
        "-o",
        "-",
    ]);
    assert_success(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("mpc.bus = ["), "{stdout}");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("wrote "),
        "stdout target should not report a file write: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn convert_overwrites_existing_text_output() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out_path = std::env::temp_dir().join(format!("powerio-cli-convert-{stamp}.m"));
    std::fs::write(&out_path, "sentinel").unwrap();

    let case = repo_file("tests/data/case9.m");
    let out = run(&[
        "convert",
        case.to_str().unwrap(),
        "--to",
        "matpower",
        "-o",
        out_path.to_str().unwrap(),
    ]);
    assert_success(&out);

    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("mpc.bus = ["), "{text}");
    assert!(!text.contains("sentinel"), "{text}");

    let _ = std::fs::remove_file(out_path);
}

#[test]
fn summary_outputs_machine_readable_json() {
    let case = repo_file("tests/data/case9.m");
    let out = run(&["summary", case.to_str().unwrap()]);
    assert_success(&out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["schema"], "powerio.summary");
    assert_eq!(value["domain"], "transmission");
    assert_eq!(value["elements"]["buses"], 9);
    assert_eq!(value["topology"]["connected_components"], 1);
}

#[test]
fn package_overwrites_existing_output_file() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out_path = std::env::temp_dir().join(format!("powerio-cli-package-{stamp}.pio.json"));
    std::fs::write(&out_path, "sentinel").unwrap();

    let case = repo_file("tests/data/case9.m");
    let out = run(&[
        "package",
        case.to_str().unwrap(),
        "-o",
        out_path.to_str().unwrap(),
    ]);
    assert_success(&out);

    let text = std::fs::read_to_string(&out_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["schema"],
        "https://powerio.dev/schema/pio-package/0.2"
    );
    assert!(!text.contains("sentinel"), "{text}");

    let _ = std::fs::remove_file(out_path);
}

#[test]
fn batch_writes_requested_matrices_rhs_and_metadata() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out_dir = std::env::temp_dir().join(format!("powerio-cli-batch-{stamp}"));

    let case = repo_file("tests/data/case9.m");
    let out = run(&[
        "batch",
        "-i",
        case.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
        "--matrices",
        "ybus_real,ybus_imag",
        "--rhs",
        "injection",
        "--scheme",
        "xb",
    ]);
    assert_success(&out);

    for name in [
        "case9_ybus_real.mtx",
        "case9_ybus_imag.mtx",
        "case9_ybus_real_rhs.mtx",
        "case9_ybus_imag_rhs.mtx",
        "case9_shunt.mtx",
        "case9_meta.json",
    ] {
        assert!(out_dir.join(name).is_file(), "missing {name}");
    }

    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("case9_meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["case_name"], "case9");
    assert_eq!(meta["n_buses"], 9);
    assert_eq!(meta["n_branches"], 9);
    assert_eq!(meta["matrices"].as_array().unwrap().len(), 2);
    assert_eq!(meta["matrices"][0]["kind"], "ybus_real");
    assert_eq!(meta["matrices"][1]["kind"], "ybus_imag");
    assert!(meta["source_sha256"].as_str().unwrap().len() == 64);

    let _ = std::fs::remove_dir_all(out_dir);
}

#[test]
fn pypsa_directory_target_requires_output_directory() {
    let case = repo_file("tests/data/case9.m");
    let out = run(&["convert", case.to_str().unwrap(), "--to", "pypsa-csv"]);
    assert_failure(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("requires `-o <output-dir>`"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn family_mismatch_exits_before_writing_output() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out_path = std::env::temp_dir().join(format!("powerio-cli-family-{stamp}.m"));
    let case = repo_file("tests/data/dist/micro/xfmr_single_phase.dss");
    let out = run(&[
        "convert",
        case.to_str().unwrap(),
        "--to",
        "matpower",
        "-o",
        out_path.to_str().unwrap(),
    ]);
    assert_failure(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no conversion path"), "stderr:\n{stderr}");
    assert!(
        !out_path.exists(),
        "failed conversion wrote {}",
        out_path.display()
    );
}

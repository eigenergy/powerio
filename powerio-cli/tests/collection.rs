use std::process::Command;
fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_powerio"))
}
#[test]
fn inspect_and_batch_select_keep_partial_results_and_deterministic_ids() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("valid.m"),
        include_bytes!("../../tests/data/case9.m"),
    )
    .unwrap();
    std::fs::write(directory.path().join("invalid.m"), b"invalid matpower").unwrap();
    let inspect = cli()
        .args(["collection", "inspect"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inventory: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(inventory["candidates"].as_array().unwrap().len(), 2);
    let output = tempfile::tempdir().unwrap();
    let parsed = cli()
        .args(["collection", "parse"])
        .arg(directory.path())
        .args([
            "--entry",
            "invalid.m",
            "--entry",
            "valid.m",
            "--from",
            "matpower",
            "-o",
        ])
        .arg(output.path())
        .output()
        .unwrap();
    assert!(!parsed.status.success());
    let report: serde_json::Value = serde_json::from_slice(&parsed.stdout).unwrap();
    assert_eq!(report["results"][0]["id"], "case-0001");
    assert_eq!(report["results"][0]["status"], "error");
    assert_eq!(report["results"][1]["status"], "parsed");
    assert!(output.path().join("case-0002.pio.json").exists());
    assert!(!output.path().join("case-0001.pio.json").exists());
}
#[test]
fn missing_entry_is_actionable_and_never_chooses_a_master() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Master.dss"), b"New Circuit.x").unwrap();
    let result = cli()
        .args(["collection", "parse"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("explicit dataset format"));
}

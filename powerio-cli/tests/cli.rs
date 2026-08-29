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
fn convert_refuses_an_existing_text_output() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out_path = std::env::temp_dir().join(format!("powerio-cli-convert-{stamp}.m"));
    std::fs::write(&out_path, "sentinel").unwrap();

    let case = repo_file("tests/data/case9.m");
    // An existing entry at the output is refused and keeps its bytes; a
    // fresh path commits.
    let out = run(&[
        "convert",
        case.to_str().unwrap(),
        "--to",
        "matpower",
        "-o",
        out_path.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already exists"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&out_path).unwrap(), "sentinel");

    let fresh = std::env::temp_dir().join(format!("powerio-cli-convert-fresh-{stamp}.m"));
    let out = run(&[
        "convert",
        case.to_str().unwrap(),
        "--to",
        "matpower",
        "-o",
        fresh.to_str().unwrap(),
    ]);
    assert_success(&out);
    let text = std::fs::read_to_string(&fresh).unwrap();
    assert!(text.contains("mpc.bus = ["), "{text}");
    let _ = std::fs::remove_file(&fresh);

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
fn summary_routes_json_inputs_through_the_classifier() {
    // The `.json` routes read the file once; the classifier's verdict
    // routes each family to its parser.
    let case = repo_file("tests/data/egret/case9.json");
    let out = run(&["summary", case.to_str().unwrap()]);
    assert_success(&out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["domain"], "transmission");
    assert_eq!(value["source_format"], "egret-json");
    assert_eq!(value["elements"]["buses"], 9);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dist_json = std::env::temp_dir().join(format!("powerio-cli-summary-{stamp}.json"));
    let dss = repo_file("tests/data/dist/micro/xfmr_single_phase.dss");
    let out = run(&[
        "convert",
        dss.to_str().unwrap(),
        "--to",
        "bmopf-json",
        "-o",
        dist_json.to_str().unwrap(),
    ]);
    assert_success(&out);
    let out = run(&["summary", dist_json.to_str().unwrap()]);
    assert_success(&out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["domain"], "distribution");
    assert_eq!(value["elements"]["buses"], 2);

    let _ = std::fs::remove_file(dist_json);
}

#[test]
fn package_refuses_an_existing_output_file() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out_path = std::env::temp_dir().join(format!("powerio-cli-package-{stamp}.pio.json"));
    std::fs::write(&out_path, "sentinel").unwrap();

    let case = repo_file("tests/data/case9.m");
    // An existing entry at the output is refused and keeps its bytes; a
    // fresh path commits.
    let out = run(&[
        "module",
        case.to_str().unwrap(),
        "-o",
        out_path.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already exists"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&out_path).unwrap(), "sentinel");

    let fresh = std::env::temp_dir().join(format!("powerio-cli-package-fresh-{stamp}.pio.json"));
    let out = run(&[
        "module",
        case.to_str().unwrap(),
        "-o",
        fresh.to_str().unwrap(),
    ]);
    assert_success(&out);
    let text = std::fs::read_to_string(&fresh).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["schema"], "powerio.module");
    assert_eq!(value["producer"]["version"], powerio::VERSION);
    let _ = std::fs::remove_file(&fresh);

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
    assert_eq!(meta["source_sha256"].as_str().unwrap().len(), 64);

    let _ = std::fs::remove_dir_all(out_dir);
}

#[test]
fn sensitivities_write_solver_metadata() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out_dir = std::env::temp_dir().join(format!("powerio-cli-sensitivities-{stamp}"));

    let case = repo_file("tests/data/case9.m");
    let out = run(&[
        "sensitivities",
        case.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
        "--solver",
        "sparse",
        "--drop-tolerance",
        "1e-10",
    ]);
    assert_success(&out);

    for name in [
        "case9_ptdf.mtx",
        "case9_lodf.mtx",
        "case9_sensitivity_meta.json",
    ] {
        assert!(out_dir.join(name).is_file(), "missing {name}");
    }

    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out_dir.join("case9_sensitivity_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["case"], "case9");
    assert_eq!(meta["sensitivity"]["requested_solver"], "sparse");
    assert_eq!(meta["sensitivity"]["solver_path"], "sparse_cholesky");
    assert_eq!(meta["sensitivity"]["drop_tolerance"], 1e-10);
    assert_eq!(meta["sensitivity"]["ptdf"]["rows"], 9);
    assert_eq!(meta["sensitivity"]["ptdf"]["cols"], 9);
    assert_eq!(meta["sensitivity"]["lodf"]["rows"], 9);
    assert_eq!(meta["sensitivity"]["lodf"]["cols"], 9);

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

#[test]
fn batch_scans_directory_recursively_and_skips_unparseable_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let nested = root.join("a").join("b").join("c");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::copy(repo_file("tests/data/case9.m"), nested.join("case9.m")).unwrap();
    std::fs::copy(
        repo_file("tests/data/dist/micro/fourwire_linecode.dss"),
        root.join("feeder.dss"),
    )
    .unwrap();
    std::fs::write(root.join("junk.json"), r#"{"not": "a case"}"#).unwrap();
    let hidden = root.join(".hidden");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::copy(repo_file("tests/data/case14.m"), hidden.join("case14.m")).unwrap();
    let out_dir = root.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::copy(repo_file("tests/data/case30.m"), out_dir.join("case30.m")).unwrap();

    let out = run(&[
        "batch",
        "-i",
        root.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);
    assert_success(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out_dir.join("case9_bprime.mtx").is_file(), "{stderr}");
    // Output files are named by the circuit name inside the case (the dss
    // fixture's circuit is "fourwire"), not by the file stem.
    assert!(
        out_dir.join("fourwire_bprime.mtx").is_file(),
        "lowered dss case missing:\n{stderr}"
    );
    assert!(stderr.contains("junk.json"), "no skip warning:\n{stderr}");
    assert!(
        !out_dir.join("case14_bprime.mtx").exists(),
        "hidden directory was scanned:\n{stderr}"
    );
    assert!(
        !out_dir.join("case30_bprime.mtx").exists(),
        "output directory was scanned:\n{stderr}"
    );
}

#[test]
fn batch_scan_discovers_bmopf_json() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let json_path = root.join("sub").join("feeder.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    let dss = repo_file("tests/data/dist/micro/fourwire_linecode.dss");
    let out = run(&[
        "convert",
        dss.to_str().unwrap(),
        "--to",
        "bmopf-json",
        "-o",
        json_path.to_str().unwrap(),
    ]);
    assert_success(&out);

    let out_dir = root.join("out");
    let out = run(&[
        "batch",
        "-i",
        root.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);
    assert_success(&out);
    assert!(
        out_dir.join("fourwire_bprime.mtx").is_file(),
        "bmopf json case missing:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn batch_scan_skips_unlowerable_dss_but_explicit_input_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dss = repo_file("tests/data/dist/micro/xfmr_single_phase.dss");
    std::fs::copy(&dss, root.join("xfmr.dss")).unwrap();
    std::fs::copy(repo_file("tests/data/case9.m"), root.join("case9.m")).unwrap();
    let out_dir = root.join("out");

    let out = run(&[
        "batch",
        "-i",
        root.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);
    assert_success(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out_dir.join("case9_bprime.mtx").is_file(), "{stderr}");
    assert!(stderr.contains("xfmr.dss"), "no skip warning:\n{stderr}");

    let out = run(&[
        "batch",
        "-i",
        dss.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);
    assert_failure(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lower") && stderr.contains("balanced"),
        "expected a lowering diagnostic:\n{stderr}"
    );
}

#[test]
fn batch_scan_with_no_case_files_reports_supported_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("notes.txt"), "nothing here").unwrap();

    let out = run(&[
        "batch",
        "-i",
        root.to_str().unwrap(),
        "-o",
        root.to_str().unwrap(),
    ]);
    assert_failure(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no case files") && stderr.contains(".epc"),
        "expected the extension list:\n{stderr}"
    );
}

#[test]
fn batch_scan_where_nothing_loads_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("junk.json"), r#"{"not": "a case"}"#).unwrap();

    let out = run(&[
        "batch",
        "-i",
        root.to_str().unwrap(),
        "-o",
        root.join("out").to_str().unwrap(),
    ]);
    assert_failure(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("loaded as case files"),
        "expected the zero-loaded error:\n{stderr}"
    );
}

#[test]
fn convert_exits_nonzero_on_a_refused_include() {
    // #275: the parse continues past a refused include and the output is
    // still written, but the run must not exit 0.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let case_dir = dir.join("case");
    std::fs::create_dir_all(&case_dir).unwrap();
    std::fs::write(
        dir.join("shared.dss"),
        "New Linecode.lc1 nphases=3 r1=0.1 x1=0.2\n",
    )
    .unwrap();
    let master = case_dir.join("master.dss");
    std::fs::write(
        &master,
        "New Circuit.c1\nRedirect ../shared.dss\nNew Line.l1 bus1=a bus2=b linecode=lc1\n",
    )
    .unwrap();
    let out_path = dir.join("out.json");

    let out = run(&[
        "convert",
        master.to_str().unwrap(),
        "--to",
        "bmopf",
        "-o",
        out_path.to_str().unwrap(),
    ]);
    assert_failure(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("READ.DSS.INCLUDE_REFUSED"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("the output is incomplete"),
        "stderr:\n{stderr}"
    );
    assert!(
        out_path.is_file(),
        "the output must still be written for inspection"
    );
}

#[cfg(unix)]
#[test]
fn convert_exits_nonzero_on_an_include_refused_through_a_symbolic_link() {
    // The lexical check passes this include: the link sits in the case
    // directory. Only the loader sees that it resolves out of it.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let case_dir = dir.join("case");
    std::fs::create_dir_all(&case_dir).unwrap();
    std::fs::write(
        dir.join("shared.dss"),
        "New Linecode.lc1 nphases=3 r1=0.1 x1=0.2\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("../shared.dss", case_dir.join("link.dss")).unwrap();
    let master = case_dir.join("master.dss");
    std::fs::write(
        &master,
        "New Circuit.c1\nRedirect link.dss\nNew Line.l1 bus1=a bus2=b linecode=lc1\n",
    )
    .unwrap();
    let out_path = dir.join("out.json");

    let out = run(&[
        "convert",
        master.to_str().unwrap(),
        "--to",
        "bmopf",
        "-o",
        out_path.to_str().unwrap(),
    ]);
    assert_failure(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("READ.DSS.INCLUDE_REFUSED"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("symbolic link"),
        "the refusal must name the route:\n{stderr}"
    );
    assert!(
        stderr.contains("references unknown linecode"),
        "the linked file must stay unread:\n{stderr}"
    );
}

#[test]
fn package_exits_nonzero_on_a_refused_include() {
    // The package carries the same `Error` finding convert fails on,
    // so the module subcommand has to fail with it too — otherwise a script
    // gating on the exit code accepts a package built from a truncated network.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let case_dir = dir.join("case");
    std::fs::create_dir_all(&case_dir).unwrap();
    std::fs::write(
        dir.join("shared.dss"),
        "New Linecode.lc1 nphases=3 r1=0.1 x1=0.2\n",
    )
    .unwrap();
    let master = case_dir.join("master.dss");
    std::fs::write(
        &master,
        "New Circuit.c1\nRedirect ../shared.dss\nNew Line.l1 bus1=a bus2=b linecode=lc1\n",
    )
    .unwrap();
    let out_path = dir.join("out.pio.json");

    let out = run(&[
        "module",
        master.to_str().unwrap(),
        "-o",
        out_path.to_str().unwrap(),
    ]);
    assert_failure(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("READ.DSS.INCLUDE_REFUSED"),
        "stderr:\n{stderr}"
    );
    assert!(
        out_path.is_file(),
        "the package must still be written for inspection"
    );
}

#[test]
fn an_unreadable_include_is_a_warning_not_a_containment_refusal() {
    // A file the OS refuses to open raises the same `PermissionDenied` kind the
    // loader uses for its own symlink-escape refusal. Only the loader's own
    // refusal is a containment failure; an ordinary unreadable include inside
    // the case directory stays a warning and the run still exits 0.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let unreadable = dir.join("shared.dss");
    std::fs::write(&unreadable, "New Linecode.lc1 nphases=3 r1=0.1 x1=0.2\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let master = dir.join("master.dss");
    std::fs::write(
        &master,
        "New Circuit.c1\nRedirect shared.dss\nNew Line.l1 bus1=a bus2=b r1=0.1 x1=0.2\n",
    )
    .unwrap();
    let out_path = dir.join("out.json");

    let out = run(&[
        "convert",
        master.to_str().unwrap(),
        "--to",
        "bmopf",
        "-o",
        out_path.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    #[cfg(unix)]
    if !nix_running_as_root() {
        assert!(
            out.status.success(),
            "an unreadable include is not an escape attempt:\n{stderr}"
        );
        assert!(
            !stderr.contains("READ.DSS.INCLUDE_REFUSED"),
            "stderr:\n{stderr}"
        );
    }
}

/// Root reads a mode 000 file, so the permission case cannot be staged there.
#[cfg(unix)]
fn nix_running_as_root() -> bool {
    std::fs::File::open("/proc/1/mem").is_ok() || std::env::var("USER").is_ok_and(|u| u == "root")
}

//! Machine-readable evidence from the same cells and grades as the PR table.

use serde_json::{Value, json};

use super::*;

fn git(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn environment(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub(super) fn render(
    matrices: &[(&str, Matrix)],
    judged: &BTreeMap<String, Judged>,
    expected: &BTreeMap<String, String>,
    failures: &[String],
) -> Value {
    let mut cells = Vec::new();
    for (family, matrix) in matrices {
        for (row, source) in matrix.sources.iter().enumerate() {
            cells.push(cell(
                family,
                source,
                None,
                &matrix.source_cells[row],
                judged,
                expected,
            ));
            for (column, target) in matrix.targets.iter().enumerate() {
                cells.push(cell(
                    family,
                    source,
                    Some(target),
                    &matrix.cells[row][column],
                    judged,
                    expected,
                ));
            }
        }
    }
    let mut counts = BTreeMap::<String, usize>::new();
    for cell in &cells {
        *counts
            .entry(cell["grade"].as_str().unwrap().to_owned())
            .or_default() += 1;
    }
    json!({
        "schema": "powerio-conversion-matrix",
        "schema_version": 1,
        "powerio_version": env!("CARGO_PKG_VERSION"),
        "provenance": {
            "repository": environment("GITHUB_REPOSITORY").unwrap_or_else(|| "eigenergy/powerio".into()),
            "tested_commit": git(&["rev-parse", "HEAD"]),
            "head_commit": environment("POWERIO_MATRIX_HEAD_SHA"),
            "base_commit": environment("POWERIO_MATRIX_BASE_SHA"),
            "worktree_dirty": git(&["status", "--porcelain", "--untracked-files=normal"]).map(|status| !status.is_empty()),
            "run_id": environment("GITHUB_RUN_ID"),
            "run_attempt": environment("GITHUB_RUN_ATTEMPT"),
            "suite": "powerio-cli/tests/conversion_matrix_report.rs"
        },
        "passed": failures.is_empty(),
        "failures": failures.iter().map(|s| sanitize_report_text(s)).collect::<Vec<_>>(),
        "counts": counts,
        "cells": cells
    })
}

fn cell(
    family: &str,
    source: &str,
    target: Option<&str>,
    cell: &Cell,
    judged: &BTreeMap<String, Judged>,
    expected: &BTreeMap<String, String>,
) -> Value {
    let key = target.map_or_else(|| source_key(source), |target| pair_key(source, target));
    let verdict = &judged[&key];
    let findings = cell.findings.iter().map(|(case, leg, diagnostic)| json!({
        "case": case,
        "leg": leg.name(),
        "code": diagnostic.code(),
        "severity": diagnostic.severity().as_str(),
        "level": grade_finding(*leg, diagnostic).ok().flatten().map(|(level, _)| level.name()),
        "message": sanitize_report_text(diagnostic.message())
    })).collect::<Vec<_>>();
    json!({
        "key": key,
        "family": family,
        "source": source,
        "target": target,
        "kind": if target.is_some() { "conversion" } else { "source_parse" },
        "grade": verdict.grade.level.name(),
        "cases": cell.cases,
        "finding_codes": verdict.grade.codes,
        "findings": findings,
        "baseline": {"expected": expected.get(&key), "observed": verdict.grade.baseline_value()},
        "passed": verdict.problems.is_empty(),
        "problems": verdict.problems.iter().map(|s| sanitize_report_text(s)).collect::<Vec<_>>(),
        "model_differences": cell.model_diffs.iter().map(|(case, difference)| json!({
            "case": case, "difference": sanitize_report_text(difference),
            "equivalence": cell.model_restatements.get(&(*case, difference.clone()))
        })).collect::<Vec<_>>(),
        "verification": {
            "byte_echo_exact": cell.echo_exact,
            "limitations": cell.verification_notes.iter().map(|(case, note)| json!({"case": case, "note": sanitize_report_text(note)})).collect::<Vec<_>>(),
            "independent_consumer": "not_run_by_this_suite"
        }
    })
}

#[test]
fn evidence_keeps_failed_checks_and_source_cells_separate() {
    let matrix = Matrix {
        sources: vec!["source"],
        targets: vec!["target"],
        groups: vec![],
        source_cells: vec![Cell::default()],
        cells: vec![vec![Cell {
            failures: vec!["electrical mismatch".into()],
            ..Cell::default()
        }]],
    };
    let mut judged = BTreeMap::new();
    for (key, cell) in matrix.keyed_cells() {
        judged.insert(
            key.clone(),
            Judged {
                key,
                grade: cell.grade(),
                problems: cell.failures.clone(),
            },
        );
    }
    let report = render(
        &[("Transmission", matrix)],
        &judged,
        &BTreeMap::new(),
        &["electrical mismatch".into()],
    );
    assert_eq!(report["cells"].as_array().unwrap().len(), 2);
    assert_eq!(report["cells"][0]["kind"], "source_parse");
    assert!(report["cells"][0]["target"].is_null());
    assert_eq!(report["cells"][1]["grade"], "failed");
    assert_eq!(report["passed"], false);
    assert_eq!(
        report["cells"][1]["verification"]["independent_consumer"],
        "not_run_by_this_suite"
    );
}

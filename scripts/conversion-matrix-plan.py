#!/usr/bin/env python3
"""Rank reproducible matrix findings; never edit code or accept a baseline."""

import argparse
import hashlib
import json
from pathlib import Path

LEVELS = {"failed": 0, "dropped": 1, "approximate": 2, "restated": 3, "exact": 4, "lossless": 5}


def load(path):
    raw = Path(path).read_bytes()
    report = json.loads(raw)
    if report.get("schema") != "powerio-conversion-matrix" or report.get("schema_version") != 1:
        raise ValueError("unsupported conversion matrix evidence schema")
    cells = report["cells"]
    if len({cell["key"] for cell in cells}) != len(cells):
        raise ValueError("duplicate matrix cell keys")
    for cell in cells:
        cases = cell["cases"]
        if cell["grade"] not in LEVELS or not isinstance(cases, list) or not all(isinstance(case, str) for case in cases):
            raise ValueError("invalid grade or coverage")
        # Failed source acquisition leaves both source and conversion cells empty.
        if not cases and not (cell["grade"] == "failed" and cell.get("passed") is False and report.get("passed") is False):
            raise ValueError("empty coverage requires a failed cell and suite")
    return report, hashlib.sha256(raw).hexdigest()


def plan(report, digest, previous=None):
    cells = {cell["key"]: cell for cell in report["cells"]}
    before = {cell["key"]: cell for cell in previous["cells"]} if previous else {}
    changes = []
    for key in sorted(cells.keys() | before.keys()) if previous else []:
        old, new = before.get(key), cells.get(key)
        if old is None or new is None:
            changes.append({"key": key, "kind": "added" if old is None else "removed"})
        elif old["grade"] != new["grade"] or old["cases"] != new["cases"] or old["finding_codes"] != new["finding_codes"]:
            changes.append({
                "key": key, "kind": "changed", "before": old["grade"], "after": new["grade"],
                "cases_before": old["cases"], "cases_after": new["cases"],
                "regression": LEVELS[new["grade"]] < LEVELS[old["grade"]],
                "coverage_reduced": bool(set(old["cases"]) - set(new["cases"])),
            })
    groups = {}
    for cell in cells.values():
        if cell["grade"] in ("lossless", "exact"):
            continue
        for code in cell["finding_codes"] or {"MODEL_DIFFERENCE_OR_CHECK_FAILURE": 1}:
            # A code family can cover unrelated fields: this is a triage group,
            # not a promise that one fix will improve every affected cell.
            candidate = groups.setdefault(code, {"code": code, "cells": [], "worst_grade": cell["grade"], "examples": []})
            candidate["cells"].append(cell["key"])
            if LEVELS[cell["grade"]] < LEVELS[candidate["worst_grade"]]:
                candidate["worst_grade"] = cell["grade"]
            for finding in cell["findings"]:
                if finding["code"] == code and len(candidate["examples"]) < 3:
                    example = {k: finding[k] for k in ("case", "leg", "message")}
                    if example not in candidate["examples"]:
                        candidate["examples"].append(example)
    candidates = sorted(groups.values(), key=lambda g: (LEVELS[g["worst_grade"]], -len(g["cells"]), g["code"]))
    return {
        "schema": "powerio-conversion-backlog", "schema_version": 1,
        "evidence_sha256": digest, "provenance": report["provenance"],
        "suite_passed": report["passed"], "counts": report["counts"],
        "comparison_available": previous is not None, "changes": changes,
        "acceptance": {
            "requires": ["minimal reproducer", "native target consumer or authoritative format semantics", "unchanged grading rules", "full affected checks", "reviewed baseline delta"],
            "automatic_baseline_update": False, "automatic_merge": False,
            "independent_consumer": "see per-test oracle results; matrix suite alone does not establish this",
        },
        "candidates": candidates,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--previous", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report, digest = load(args.evidence)
    previous = load(args.previous)[0] if args.previous else None
    args.output.write_text(json.dumps(plan(report, digest, previous), indent=2) + "\n")


if __name__ == "__main__":
    main()

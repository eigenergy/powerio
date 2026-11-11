"""The repair queue must preserve failures, reduced coverage and provenance."""

import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("planner", Path(__file__).with_name("conversion-matrix-plan.py"))
planner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(planner)


def report():
    return {"schema": "powerio-conversion-matrix", "schema_version": 1, "passed": False,
            "provenance": {"tested_commit": "abc", "worktree_dirty": True}, "counts": {"dropped": 1},
            "cells": [{"key": "A -> B", "grade": "dropped", "cases": ["one", "two"],
                       "finding_codes": {"EMIT.B.FIELD_DROPPED": 1}, "findings": []}]}


class PlannerTests(unittest.TestCase):
    def test_no_greenwashing_failed_run(self):
        result = planner.plan(report(), "digest")
        self.assertFalse(result["suite_passed"])
        self.assertTrue(result["provenance"]["worktree_dirty"])
        self.assertFalse(result["acceptance"]["automatic_baseline_update"])
        self.assertEqual(result["evidence_sha256"], "digest")

    def test_removed_or_shrunk_coverage_is_visible_even_when_greener(self):
        previous = report()
        previous["cells"].append({**previous["cells"][0], "key": "A -> C"})
        current = report()
        current["cells"][0].update(grade="lossless", cases=["one"], finding_codes={})
        changes = planner.plan(current, "digest", previous)["changes"]
        self.assertTrue(changes[0]["coverage_reduced"])
        self.assertEqual(changes[1]["kind"], "removed")

    def test_regressions_sort_ahead_of_improvement_opportunities(self):
        current = report()
        current["cells"].append({**current["cells"][0], "key": "C -> D", "grade": "failed", "finding_codes": {"FAIL": 1}})
        self.assertEqual(planner.plan(current, "digest")["candidates"][0]["code"], "FAIL")

    def test_failed_source_without_cases_still_writes_backlog(self):
        previous = report()
        previous["cells"].append({**previous["cells"][0], "key": "A source"})
        previous["counts"] = {"dropped": 2}
        current = report()
        current["counts"] = {"failed": 2}
        current["failures"] = ["open source fixture: missing input"]
        current["cells"] = [{
            "key": key, "kind": kind, "grade": "failed", "cases": [],
            "passed": False, "problems": current["failures"],
            "finding_codes": {}, "findings": [],
        } for key, kind in (("A source", "source_parse"), ("A -> B", "conversion"))]
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Path(temporary) / "matrix.json"
            before = Path(temporary) / "previous.json"
            output = Path(temporary) / "backlog.json"
            raw = (json.dumps(current) + "\n").encode()
            evidence.write_bytes(raw)
            before.write_text(json.dumps(previous))
            result = subprocess.run([
                sys.executable, str(Path(__file__).with_name("conversion-matrix-plan.py")),
                str(evidence), "--previous", str(before), "--output", str(output),
            ], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            backlog = json.loads(output.read_text())
            self.assertFalse(backlog["suite_passed"])
            self.assertEqual(backlog["counts"], {"failed": 2})
            self.assertEqual(backlog["provenance"], current["provenance"])
            self.assertEqual(backlog["evidence_sha256"], hashlib.sha256(raw).hexdigest())
            self.assertEqual(evidence.read_bytes(), raw)
            self.assertEqual(backlog["candidates"], [{
                "code": "MODEL_DIFFERENCE_OR_CHECK_FAILURE",
                "cells": ["A source", "A -> B"], "worst_grade": "failed", "examples": [],
            }])
            self.assertEqual(len(backlog["changes"]), 2)
            self.assertTrue(all(change["regression"] for change in backlog["changes"]))
            self.assertTrue(all(change["coverage_reduced"] for change in backlog["changes"]))
            self.assertTrue(all(change["cases_after"] == [] for change in backlog["changes"]))

    def test_empty_cases_cannot_claim_a_successful_cell_or_suite(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "report.json"
            for grade in planner.LEVELS:
                for cell_passed, suite_passed in ((True, False), (False, True), (True, True), (False, False)):
                    if grade == "failed" and not cell_passed and not suite_passed:
                        continue
                    with self.subTest(grade=grade, cell_passed=cell_passed, suite_passed=suite_passed):
                        value = report()
                        value["passed"] = suite_passed
                        value["cells"][0].update(grade=grade, cases=[], passed=cell_passed)
                        path.write_text(json.dumps(value))
                        with self.assertRaises(ValueError):
                            planner.load(path)

    def test_unknown_schema_duplicate_cells_and_unknown_grades_refused(self):
        variants = [report() for _ in range(3)]
        variants[0]["schema_version"] = 2
        variants[1]["cells"] *= 2
        variants[2]["cells"][0]["grade"] = "green"
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "report.json"
            for value in variants:
                path.write_text(json.dumps(value))
                with self.assertRaises(ValueError):
                    planner.load(path)


if __name__ == "__main__":
    unittest.main()

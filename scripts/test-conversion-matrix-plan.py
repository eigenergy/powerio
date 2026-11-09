"""The repair queue must preserve failures, reduced coverage and provenance."""

import importlib.util
import json
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

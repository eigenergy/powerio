#!/usr/bin/env python3
"""The override must never change main validation or inject checkout outputs."""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("companion", Path(__file__).with_name("julia-companion.py"))
companion = importlib.util.module_from_spec(spec)
spec.loader.exec_module(companion)


class CompanionTests(unittest.TestCase):
    def test_selection_and_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "companion.json"
            self.assertEqual(companion.resolve("pull_request", path), {"enabled": "false"})
            data = {"repository": "frederikgeth/PowerIO.jl", "sha": "a" * 40,
                    "pull_request": "https://github.com/eigenergy/PowerIO.jl/pull/165"}
            path.write_text(json.dumps(data))
            self.assertEqual(companion.resolve("pull_request", path),
                             {"enabled": "true", "repository": data["repository"], "ref": data["sha"]})
            for key, value in [("repository", "owner/PowerIO.jl\nref=main"),
                               ("repository", "https://example.org/PowerIO.jl"),
                               ("sha", "main"), ("sha", "a" * 40 + "\nenabled=false"),
                               ("pull_request", "https://example.org/165"), ("sha", None)]:
                with self.subTest(key=key, value=value):
                    path.write_text(json.dumps({**data, key: value}))
                    with self.assertRaises(ValueError):
                        companion.resolve("pull_request", path)
            # Even a broken PR override must not affect main or release runs.
            path.write_text("not json")
            for event in ["push", "workflow_dispatch"]:
                self.assertEqual(companion.resolve(event, path), {"enabled": "false"})
            with self.assertRaises(ValueError):
                companion.resolve("pull_request", path)


if __name__ == "__main__":
    unittest.main()

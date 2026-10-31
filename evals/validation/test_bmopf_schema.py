"""Regression checks for the external BMOPF validation harness."""

import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import validate_bmopf_schema as harness


class MultinetworkValidation(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.validators = {
            version: harness.schema_validator(path)
            for version, path in harness.SCHEMAS.items()
        }
        cls.schema_ids = {
            version: json.loads(path.read_text())["$id"]
            for version, path in harness.SCHEMAS.items()
        }
        cls.case = Path("tests/data/dist/pmd/case3_balanced_multinetwork.json")

    def test_every_snapshot_is_validated(self):
        emit = harness.powerio.emit
        with patch.object(harness.powerio, "emit", wraps=emit) as observed:
            failures = harness.validate_case(self.validators, self.schema_ids, self.case)
        self.assertEqual(failures, [])
        self.assertEqual(observed.call_count, 2)

    def test_later_snapshot_failure_is_reported_with_its_label(self):
        emit = harness.powerio.emit
        calls = 0

        def corrupt_second_snapshot(*args, **kwargs):
            nonlocal calls
            calls += 1
            result = emit(*args, **kwargs)
            if calls != 2:
                return result
            document = json.loads(result.text)
            del document["bus"]
            return SimpleNamespace(text=json.dumps(document))

        with patch.object(harness.powerio, "emit", side_effect=corrupt_second_snapshot):
            failures = harness.validate_case(self.validators, self.schema_ids, self.case)
        self.assertEqual(calls, 2)
        self.assertTrue(failures)
        self.assertTrue(all(failure.startswith("2 ") for failure in failures), failures)
        self.assertTrue(any("bus" in failure for failure in failures), failures)


if __name__ == "__main__":
    unittest.main()

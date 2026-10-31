"""Regression coverage for the validation harness's snapshot traversal."""
import json
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import validate_bmopf_schema as validation


class SnapshotValidationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.validators = {
            version: validation.schema_validator(path)
            for version, path in validation.SCHEMAS.items()
        }
        cls.schema_ids = {
            version: json.loads(path.read_text())["$id"]
            for version, path in validation.SCHEMAS.items()
        }
        cls.case = Path("tests/data/dist/pmd/case3_balanced_multinetwork.json")

    def test_every_snapshot_is_emitted_and_validated(self):
        count = len(validation.powerio.parse(self.case).value)
        self.assertGreater(count, 1)
        emit = validation.powerio.emit
        with patch.object(validation.powerio, "emit", wraps=emit) as spy:
            errors = validation.validate_case(self.validators, self.schema_ids, self.case)
        self.assertEqual(errors, [])
        self.assertEqual(spy.call_count, count)

    def test_invalid_later_snapshot_is_reported_with_its_index(self):
        emit = validation.powerio.emit
        calls = 0

        def corrupt_second(module, format):
            nonlocal calls
            output = emit(module, format)
            calls += 1
            if calls == 2:
                doc = json.loads(output.text)
                doc["bus"] = "invalid bus table"
                return SimpleNamespace(text=json.dumps(doc))
            return output

        with patch.object(validation.powerio, "emit", side_effect=corrupt_second):
            errors = validation.validate_case(self.validators, self.schema_ids, self.case)
        self.assertTrue(errors)
        self.assertTrue(all(error.startswith("snapshot[1]:") for error in errors))


if __name__ == "__main__":
    unittest.main()

"""Regressions for false-green coverage and RDF/XML validation bypasses."""

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import schema_check as sc

CIM = "http://iec.ch/TC57/CIM100#"
RDF = f'<rdf:RDF xmlns:rdf="{sc.RDF}" xmlns:cim="{CIM}">{{}}</rdf:RDF>'


class VocabularyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.vocab = sc.Profile(sc.VOCABULARY_PROFILE)
        cls.profile = sc.Profile(sc.PROFILE)

    def findings(self, body):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "case.xml"
            path.write_text(RDF.format(body))
            return sc.check_document(path, self.vocab, self.profile).counts

    def test_nested_primitive_is_rejected(self):
        got = self.findings('<cim:PowerTransformerEnd rdf:ID="end"><cim:PowerTransformerEnd.ratedU><cim:Unknown/></cim:PowerTransformerEnd.ratedU></cim:PowerTransformerEnd>')
        self.assertEqual(got[f"{sc.NESTED_VALUE} PowerTransformerEnd.ratedU"], 1)

    def test_nested_wrong_association_target_is_rejected(self):
        got = self.findings('<cim:Terminal rdf:ID="t"><cim:Terminal.ConductingEquipment><cim:BaseVoltage rdf:ID="v"/></cim:Terminal.ConductingEquipment></cim:Terminal>')
        self.assertEqual(got[f"{sc.NESTED_VALUE} Terminal.ConductingEquipment"], 1)

    def test_nested_enumeration_is_rejected(self):
        got = self.findings('<cim:PowerTransformerEnd rdf:ID="end"><cim:PowerTransformerEnd.connectionKind><cim:Unknown/></cim:PowerTransformerEnd.connectionKind></cim:PowerTransformerEnd>')
        self.assertEqual(got[f"{sc.NESTED_VALUE} PowerTransformerEnd.connectionKind"], 1)

    def test_flat_literal_and_reference_controls(self):
        got = self.findings('<cim:PowerTransformerEnd rdf:ID="end"><cim:PowerTransformerEnd.ratedU>1000</cim:PowerTransformerEnd.ratedU></cim:PowerTransformerEnd><cim:Terminal rdf:ID="t"><cim:Terminal.ConductingEquipment rdf:resource="#line"/></cim:Terminal><cim:ACLineSegment rdf:ID="line"/>')
        self.assertFalse(any(key.startswith("VOCAB.") for key in got))

    def test_flat_invalid_literal_and_target_still_fail(self):
        got = self.findings('<cim:PowerTransformerEnd rdf:ID="end"><cim:PowerTransformerEnd.ratedU>no</cim:PowerTransformerEnd.ratedU></cim:PowerTransformerEnd><cim:Terminal rdf:ID="t"><cim:Terminal.ConductingEquipment rdf:resource="#v"/></cim:Terminal><cim:BaseVoltage rdf:ID="v"/>')
        self.assertEqual(got[f"{sc.LITERAL_INVALID} PowerTransformerEnd.ratedU"], 1)
        self.assertEqual(got[f"{sc.REFERENCE_TYPE} Terminal.ConductingEquipment -> BaseVoltage"], 1)


class CoverageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.fixtures = self.root / "fixtures"
        (self.fixtures / "micro").mkdir(parents=True)
        self.source = self.fixtures / "micro" / "case.dss"
        self.source.write_text("new circuit.control\n")
        self.expected = self.root / "expected.json"
        self.doc = {"cases": {f"{self.source} output": {}}}
        self.save()
        self.mode = self.root / "mode"
        self.mode.write_text("ok")
        self.cli = self.root / "powerio"
        # Real subprocess boundary: reproduce serialize/convert failures,
        # including a successful process which forgets its output file.
        self.cli.write_text(f'''#!{sys.executable}
import pathlib, sys
mode = pathlib.Path({str(self.mode)!r}).read_text()
stage = sys.argv[1]
out = pathlib.Path(sys.argv[sys.argv.index('-o') + 1])
if mode == stage + '_fail':
    sys.stderr.write('PARSE.TEST.REFUSED')
    sys.exit(2)
if mode == stage + '_missing':
    sys.exit(0)
out.write_text('{{}}' if stage == 'serialize' else {RDF.format('')!r})
if mode == 'serialize_errors_with_ir' and stage == 'serialize':
    sys.exit(1)
''')
        self.cli.chmod(0o755)

    def save(self):
        self.expected.write_text(json.dumps(self.doc))

    def run_check(self, *args):
        stream = io.StringIO()
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(sc, "FIXTURE_ROOT", self.fixtures))
            stack.enter_context(patch.object(sc, "FIXTURE_DIRS", ["micro"]))
            stack.enter_context(patch.object(sc, "EXPECTED", self.expected))
            stack.enter_context(patch.object(sys, "argv", ["schema_check", "--powerio", str(self.cli), "--corpus", str(self.root / "absent"), *args]))
            stack.enter_context(contextlib.redirect_stdout(stream))
            result = sc.main()
        return result, stream.getvalue()

    def test_complete_fixture_only_run_passes(self):
        code, out = self.run_check("--fixtures-only")
        self.assertEqual(code, 0, out)
        self.assertIn("1 documents checked", out)
        self.assertIn("not requested", out)

    def test_missing_fixture_with_empty_baseline_fails(self):
        self.source.unlink()
        code, out = self.run_check("--fixtures-only")
        self.assertEqual(code, 1, out)
        self.assertIn("expected case was not checked", out)

    def test_missing_fixture_directory_fails(self):
        self.source.unlink()
        self.source.parent.rmdir()
        self.assertEqual(self.run_check("--fixtures-only")[0], 1)

    def test_serialization_and_conversion_failures_cannot_disappear(self):
        for mode in ("serialize_fail", "serialize_missing", "convert_fail", "convert_missing"):
            with self.subTest(mode=mode):
                self.mode.write_text(mode)
                code, out = self.run_check("--fixtures-only")
                self.assertEqual(code, 1, out)
                self.assertIn("CONVERT.REFUSED", out)

    def test_recorded_refusal_passes_but_changed_reason_fails(self):
        self.doc["cases"][f"{self.source} output"] = {"CONVERT.REFUSED PARSE.TEST.REFUSED": 1}
        self.save()
        self.mode.write_text("convert_fail")
        self.assertEqual(self.run_check("--fixtures-only")[0], 0)
        self.mode.write_text("serialize_fail")
        self.assertEqual(self.run_check("--fixtures-only")[0], 1)

    def test_serializer_error_with_ir_still_reaches_conversion(self):
        self.mode.write_text("serialize_errors_with_ir")
        self.assertEqual(self.run_check("--fixtures-only")[0], 0)

    def test_new_unbaselined_case_cannot_pass_with_zero_findings(self):
        (self.source.parent / "new.dss").write_text("new circuit.other\n")
        self.assertEqual(self.run_check("--fixtures-only")[0], 1)

    def test_non_case_skips_are_explicit(self):
        noncase = self.source.parent / "results.json"
        noncase.write_text("{}")
        self.doc["fixture_skips"] = {str(noncase): "oracle result, not a network"}
        self.save()
        code, out = self.run_check("--fixtures-only")
        self.assertEqual(code, 0, out)
        self.assertIn("skip non-case", out)
        self.assertIn("1 documents checked", out)

    def test_missing_corpus_is_optional_and_reported(self):
        code, out = self.run_check()
        self.assertEqual(code, 0, out)
        self.assertIn("0/6 feeders present", out)

    def test_required_corpus_cannot_be_skipped(self):
        self.assertEqual(self.run_check("--require-corpus")[0], 1)

    def test_update_expected_cannot_erase_a_missing_fixture(self):
        before = self.expected.read_text()
        self.source.unlink()
        self.assertEqual(self.run_check("--fixtures-only", "--update-expected")[0], 1)
        self.assertEqual(self.expected.read_text(), before)


if __name__ == "__main__":
    unittest.main()

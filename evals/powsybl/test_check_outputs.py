"""Mutation controls for the strict generated-CGMES diagnostic inventory."""

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from check_outputs import (
    CGMES_VALUE_SUBSTITUTED_PREFIX,
    CIM100,
    RDF,
    check_generated_cgmes_substitution_diagnostics,
)


class GeneratedCgmesDiagnosticTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        root = Path(self.directory.name)
        self.ir = root / "source.pio.json"
        self.log = root / "emit.log"
        self.xml_dir = root / "cgmes"
        self.xml_dir.mkdir()
        # Fixed UUIDv5 vector, independently derived from URL namespace
        # https://powerio.dev/cgmes and name bus:bus/B1.
        self.mrid = "edef51b0-c024-5be5-8368-0e3600564030"
        component = {"component_type": "bus", "local_id": "B1"}
        detailed = {
            "bus_breaker_buses": [{"calculated_bus": 1, "component": component}],
            "component_metadata": [{
                "component": component,
                "external_identifiers": [{"authority": "CGMES", "value": "B1"}],
            }],
        }
        self.ir.write_text(json.dumps({"value": {"data": {
            "buses": [{"id": 1, "uid": "B1"}],
            "detailed_connectivity": detailed,
        }}}), encoding="utf-8")
        self.legacy = (
            CGMES_VALUE_SUBSTITUTED_PREFIX
            + "component `bus/B1` has non-UUID CGMES identifier `B1`; "
            + "fresh CGMES uses a deterministic UUID"
        )
        self.bus = (
            CGMES_VALUE_SUBSTITUTED_PREFIX
            + "bus 1 uid `B1` is replaced by CGMES TopologicalNode mRID "
            + f"`{self.mrid}`"
        )
        self.log.write_text(self.legacy + "\n" + self.bus + "\n", encoding="utf-8")
        self.write_xml(self.mrid)

    def write_xml(self, mrid):
        (self.xml_dir / "TP.xml").write_text(
            f'<rdf:RDF xmlns:rdf="{RDF}" xmlns:cim="{CIM100}">'
            f'<cim:TopologicalNode rdf:ID="_{mrid}"/>'
            '</rdf:RDF>', encoding="utf-8",
        )

    def check(self):
        with contextlib.redirect_stdout(io.StringIO()):
            return check_generated_cgmes_substitution_diagnostics(
                self.ir, self.log, self.xml_dir, "mutation control",
            )

    def test_exact_inventory_and_emitted_identity_pass(self):
        self.check()

    def test_missing_extra_or_altered_diagnostic_fails(self):
        for lines in (
            [self.legacy],
            [self.bus],
            [self.legacy, self.bus, self.bus],
            [self.legacy, self.bus.replace(self.mrid, "wrong-mrid")],
            [self.legacy, self.bus.replace("uid `B1`", "uid `wrong-bus`")],
        ):
            with self.subTest(lines=lines):
                self.log.write_text("\n".join(lines), encoding="utf-8")
                with self.assertRaisesRegex(AssertionError, "exact CGMES identifier substitution"):
                    self.check()

    def test_correct_diagnostic_cannot_hide_wrong_emitted_identity(self):
        self.write_xml("00000000-0000-4000-8000-000000000000")
        with self.assertRaisesRegex(AssertionError, "emitted TopologicalNode mRIDs"):
            self.check()


if __name__ == "__main__":
    unittest.main()

"""Checks that the validation adapter rejects false electrical agreement."""

import copy
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np
from coordinate_oracle import check as check_coordinates
from coordinate_oracle import expected as expected_coordinates
from object_oracle import source_graph
from oracle import add, analytic, cases, compare, parse_complex, recorded


class OracleTests(unittest.TestCase):
    def test_object_graph_rejects_ambiguous_or_missing_identity(self):
        node = {
            "key": 0,
            "name": "source",
            "class": "node",
            "parent": None,
            "references": {},
        }
        graph, classes = source_graph([node])
        self.assertEqual(graph["source"]["class"], "node")
        self.assertEqual(classes, {"node": 1})
        for other in (
            dict(node),
            dict(node, key=1),
            dict(node, key=1, name="sink", parent=2),
            dict(node, key=1, name="sink", references={"from": 2}),
        ):
            with self.assertRaises(ValueError):
                source_graph([node, other])

    def test_coordinate_negative_controls(self):
        check_coordinates(expected_coordinates())
        declared = copy.deepcopy(expected_coordinates())
        declared["line"] = {"latitude": 10, "longitude": 20}
        check_coordinates(declared, declared=True)
        with self.assertRaises(ValueError):
            check_coordinates(expected_coordinates(), declared=True)
        declared["copied"]["latitude"] = 2
        with self.assertRaises(ValueError):
            check_coordinates(declared, declared=True)
        for defect in (
            "axis",
            "missing_zero",
            "invented_zero",
            "nonfinite",
            "declared_line",
        ):
            with self.subTest(defect=defect):
                actual = copy.deepcopy(expected_coordinates())
                if defect == "axis":
                    actual["northern"] = {"latitude": -120.25, "longitude": 45.5}
                elif defect == "missing_zero":
                    del actual["zero"]["latitude"]
                elif defect == "invented_zero":
                    actual["absent"]["latitude"] = 0
                elif defect == "nonfinite":
                    actual["source"]["latitude"] = float("nan")
                else:
                    actual["line"] = {"latitude": 10, "longitude": 20}
                with self.assertRaises(ValueError):
                    check_coordinates(actual)

    def test_complex_conventions(self):
        for text, expected in (
            ("3-4j", 3 - 4j),
            ("3+4i", 3 + 4j),
            ("240-120d", -120 - 120j * math.sqrt(3)),
            ("2+1.5707963267948966r", 2j),
            ("1e-3-2e-4j", 0.001 - 0.0002j),
        ):
            self.assertAlmostEqual(parse_complex(text), expected)
        for text in ("nan", "inf+0j", "bad", "1+2x", "1+2+3d"):
            with self.assertRaises(ValueError):
                parse_complex(text)

    def test_hand_calculated_resistor_circuit(self):
        actual = analytic(np.array([12 + 0j]), np.array([[2 + 0j]]), np.array([4 + 0j]))
        # 2 A, 8 V load, 24 W entering and 16 W leaving the line, 8 W loss.
        expected = {}
        for key, unit, value in (
            ("source.A.voltage", "V", 12),
            ("sink.A.voltage", "V", 8),
            ("line.from.A.current", "A", 2),
            ("line.to.A.current", "A", -2),
            ("line.from.A.power", "VA", 24),
            ("line.to.A.power", "VA", -16),
            ("line.loss", "VA", 8),
            ("line.z.AA", "ohm", 2),
        ):
            add(expected, key, unit, complex(value))
        self.assertTrue(compare(expected, actual)["passed"])

    def test_negative_controls(self):
        reference = analytic(*cases()["coupled_unbalanced"])
        self.assertTrue(compare(reference, reference)["passed"])
        for defect in (
            "phase",
            "polarity",
            "unit",
            "missing",
            "extra",
            "nan",
            "ground",
        ):
            with self.subTest(defect=defect):
                actual = copy.deepcopy(reference)
                if defect == "phase":
                    a, b = "sink.A.voltage", "sink.B.voltage"
                    actual[a], actual[b] = actual[b], actual[a]
                elif defect == "polarity":
                    actual["line.to.A.current"]["value"] = [
                        -v for v in actual["line.to.A.current"]["value"]
                    ]
                elif defect == "unit":
                    actual["sink.A.voltage"]["value"] = [
                        v / 1000 for v in actual["sink.A.voltage"]["value"]
                    ]
                elif defect == "missing":
                    del actual["line.loss"]
                elif defect == "extra":
                    add(actual, "sink.N.voltage", "V", 0j)
                elif defect == "nan":
                    actual["line.loss"]["value"][0] = float("nan")
                elif defect == "ground":
                    # Same phase-phase voltages, but a physically different
                    # earth reference. No implicit common-mode alignment.
                    for phase in "ABC":
                        actual[f"sink.{phase}.voltage"]["value"][0] += 3
                self.assertFalse(compare(reference, actual)["passed"])

    def test_empty_duplicate_and_unit_mismatch(self):
        self.assertFalse(compare({}, {})["passed"])
        a = {}
        add(a, "neutral", "V", 0j)
        with self.assertRaises(ValueError):
            add(a, "neutral", "V", 0j)
        b = copy.deepcopy(a)
        b["neutral"]["unit"] = "kV"
        self.assertFalse(compare(a, b)["passed"])

    def test_recorder_requires_data_and_uses_final_sample(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "record.csv"
            path.write_text("# timestamp,voltage_A\n")
            with self.assertRaises(ValueError):
                recorded(path)
            path.write_text("# timestamp,voltage_A\nt0,1+0j\nt1,2+90d\n")
            self.assertAlmostEqual(recorded(path)["voltage_A"], 2j)


if __name__ == "__main__":
    unittest.main()

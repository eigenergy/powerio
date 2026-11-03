"""Checks that the validation adapter rejects false electrical agreement."""

import copy
import math
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
from coordinate_oracle import check as check_coordinates
from coordinate_oracle import expected as expected_coordinates
from group_recorders import recorders
from ideal_switch_oracle import (
    parameters as ideal_parameters,
)
from ideal_switch_oracle import (
    reference as ideal_reference,
)
from ideal_switch_oracle import (
    variants as ideal_variants,
)
from line_shunt_oracle import reference as shunt_reference
from object_oracle import source_graph
from oracle import add, analytic, cases, compare, parse_complex, recorded
from pair_load_oracle import reference as pair_reference
from pair_load_oracle import variants as pair_variants
from quantity_oracle import check_values
from quantity_oracle import reference as quantity_reference
from shunt_measurement import measurements as shunt_measurements
from switch_roundtrip_oracle import impedance_run


class OracleTests(unittest.TestCase):
    def test_feeder_recorders_preserve_all_targets_and_native_string_bound(self):
        properties = {"node": ["voltage_A", "voltage_B"]}
        objects = {f"node_{i:04d}": {"class": "node"} for i in range(100)}
        text, spec, outputs = recorders(properties, objects, "pnnl")
        self.assertEqual(spec, properties)
        self.assertEqual(outputs, ("node-voltage_A.csv", "node-voltage_B.csv"))
        self.assertIn('group "class=node"; property voltage_A;', text)
        self.assertIn('group "class=node"; property voltage_B;', text)
        text, spec, outputs = recorders(properties, objects, "arras")
        self.assertEqual(outputs, ("records/.",))
        targets = [field for chunk in spec["chunks"] for field in chunk]
        self.assertEqual(
            targets,
            [f"node_{i:04d}:voltage_{phase}" for i in range(100) for phase in "AB"],
        )
        self.assertEqual(len(targets), len(set(targets)))
        for chunk in spec["chunks"]:
            self.assertTrue(chunk)
            self.assertLessEqual(len(",".join(chunk)), 900)
        self.assertEqual(text.count("object multi_recorder"), len(spec["chunks"]))


    def test_line_shunt_reference_rejects_omission_and_swapped_ends(self):
        for case in ("source_only", "sink_only", "unequal_lossy", "reversed_lossy"):
            for edited in (False, True):
                expected, residual = shunt_reference(case, edited)
                self.assertLess(residual, 1e-9)
                for defect in ("omit", "swap"):
                    wrong, _ = shunt_reference(case, edited, defect)
                    self.assertFalse(compare(expected, wrong)["passed"])

    def test_native_shunt_readback_stamps_pair_and_earth_currents(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / "model.glm").write_text(
                'module powerflow { solver_method NR; } object load { name "line__glm_shunt_from"; parent "source"; }'
            )
            (directory / "line__glm_shunt_from.csv").write_text(
                "# timestamp,voltage_A,voltage_B,voltage_C,constant_impedance_AB,constant_impedance_BC,constant_impedance_CA,constant_impedance_AN,constant_impedance_BN,constant_impedance_CN\nt0,12+0j,8+0j,0+0j,2+0j,0+0j,0+0j,4+0j,0+0j,0+0j\n"
            )
            result = list(shunt_measurements(directory, "ABC"))
            self.assertEqual(
                result,
                [
                    (
                        "line__glm_shunt_from",
                        "source",
                        [12 + 0j, 8 + 0j, 0j],
                        [5 + 0j, -2 + 0j, 0j],
                    )
                ],
            )

    def test_phase_pair_reference_detects_wrong_connections_and_fixed_current(self):
        for name, terms in pair_variants().items():
            for edited in (False, True):
                with self.subTest(case=name, edited=edited):
                    result, metadata = pair_reference(terms, edited)
                    self.assertLess(metadata["kcl_residual_amperes"], 1e-9)
                    wrong, _ = pair_reference(terms, edited, wrong_connection=True)
                    self.assertFalse(compare(result, wrong)["passed"])
                    if any(law == "current" for law, _, _ in terms):
                        fixed, _ = pair_reference(terms, edited, fixed_current=True)
                        self.assertFalse(compare(result, fixed)["passed"])

    def test_passive_pair_terminal_reversal_is_electrically_identical(self):
        # A negative control must move the connection, not merely reverse the
        # two terminals of a passive impedance (which leaves its stamp equal).
        forward, _ = pair_reference([("impedance", "AB", 80 + 20j)])
        reverse, _ = pair_reference([("impedance", "BA", 80 + 20j)])
        self.assertTrue(compare(forward, reverse)["passed"])

    def test_ideal_contact_reference_satisfies_zip_kcl_and_detects_epsilon(self):
        for spec in ideal_variants():
            for edited in (False, True):
                _, phases, closed, zip_load, coupled, _ = spec
                _, _, loads, power, amps, _, _ = ideal_parameters(
                    phases, closed, zip_load, coupled, edited
                )
                result = ideal_reference(spec, edited)
                for i, phase in enumerate(phases):
                    v = complex(*result[f"sink.{phase}"]["value"])
                    current = complex(*result[f"contact.current_in_{phase}"]["value"])
                    if phase in closed:
                        demand = (
                            v / loads[i]
                            + amps[i] * v / abs(v)
                            + (power[i] / v).conjugate()
                        )
                        self.assertLess(abs(current - demand), 1e-10)
                        self.assertEqual(
                            result[f"middle.{phase}"], result[f"sink.{phase}"]
                        )
                    else:
                        self.assertEqual((v, current), (0j, 0j))
                self.assertFalse(
                    compare(ideal_reference(spec, edited, 1e-4), result)["passed"]
                )

    def test_successful_engine_exit_does_not_hide_a_nonfinite_switch_result(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "nonfinite"
            with (
                patch("switch_roundtrip_oracle.run_glm", return_value="pinned engine"),
                patch("switch_oracle.snapshot", side_effect=ValueError("nannand")),
            ):
                result = impedance_run("pnnl", directory, "input", {}, {}, None)
            self.assertFalse(result["passed"])
            self.assertEqual(result["version"], "pinned engine")
            self.assertEqual(result["error"], "nannand")
            self.assertEqual(len(result["input_sha256"]), 64)
            self.assertEqual((directory / "model.glm").read_text(), "input")

    def test_quantity_comparator_rejects_units_notation_and_missing_values(self):
        reference = quantity_reference()
        check_values(reference, reference, 1e-12)
        for key, wrong in [
            ("probe.z11", 5000),
            ("probe.z32", 5),
            ("probe.z21", 5 + 0.927295218j),
            ("probe.c11", float("nan")),
        ]:
            actual = dict(reference, **{key: wrong})
            with self.assertRaises(ValueError):
                check_values(actual, reference, 1e-12)
        with self.assertRaises(ValueError):
            check_values({}, reference, 1e-12)

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
        for text in (
            "nan",
            "inf+0j",
            "bad",
            "1+2x",
            "1+2+3d",
            "nannand",
            "-nan-nand",
            "-nan-nanj",
        ):
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

"""The per-phase comparator must reject structurally or electrically wrong data."""

import copy
import unittest

from load_phase_oracle import compare


class LoadPhaseComparisonTests(unittest.TestCase):
    def test_identity_polarity_and_nonfinite_controls(self):
        expected = dict(
            branches={"1.4": dict(voltage=230 + 2j, current=2 - 1j, power=458 + 234j)},
            conductor_currents={1: 2 - 1j, 4: -2 + 1j},
        )
        self.assertTrue(compare(expected, expected)["passed"])
        for defect in ("extra", "missing", "polarity", "nonfinite", "return"):
            with self.subTest(defect=defect):
                actual = copy.deepcopy(expected)
                if defect == "extra":
                    actual["branches"]["2.4"] = actual["branches"]["1.4"]
                elif defect == "missing":
                    actual["branches"].clear()
                elif defect == "polarity":
                    actual["branches"]["1.4"]["current"] *= -1
                elif defect == "nonfinite":
                    actual["branches"]["1.4"]["voltage"] = complex("inf")
                else:
                    actual["conductor_currents"][4] *= -1
                self.assertFalse(compare(expected, actual)["passed"])

    def test_empty_and_nonfinite_references_cannot_pass(self):
        empty = dict(branches={}, conductor_currents={})
        self.assertFalse(compare(empty, empty)["passed"])
        nonfinite = dict(
            branches={"1.0": dict(voltage=complex("inf"), current=1j, power=1j)},
            conductor_currents={1: 1j, 0: -1j},
        )
        self.assertFalse(compare(nonfinite, nonfinite)["passed"])


if __name__ == "__main__":
    unittest.main()

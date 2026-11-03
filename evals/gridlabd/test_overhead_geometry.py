"""Independent boundary-condition checks for the geometry calibration reference."""

import unittest

import numpy as np
from oracle import compare
from overhead_geometry_oracle import loads, primitives, reduced, reference


class OverheadGeometryTests(unittest.TestCase):
    def test_reduced_ports_equal_full_four_conductor_grounded_neutral_circuit(self):
        z, y = primitives("ABCN")
        series = np.linalg.inv(z)
        source = np.r_[
            240 * np.exp(1j * np.array([0, -2 * np.pi / 3, 2 * np.pi / 3])), 0
        ]
        sink = np.zeros(4, dtype=complex)
        sink[:3] = np.linalg.solve(
            series[:3, :3] + y[:3, :3] / 2 + np.diag(1 / loads()),
            series[:3] @ source,
        )
        current = series @ (source - sink)
        from_current = current + y @ source / 2
        to_current = -current + y @ sink / 2
        self.assertLess(max(abs(to_current[:3] + sink[:3] / loads())), 1e-10)
        self.assertGreater(abs(from_current[3]), 1)
        expected, _ = reference("ABCN", 60, True)
        for i, p in enumerate("ABC"):
            for key, actual in [
                (f"sink.{p}.voltage", sink[i]),
                (f"line.from.{p}.current", from_current[i]),
                (f"line.to.{p}.current", to_current[i]),
            ]:
                re, im = expected[key]["value"]
                self.assertLess(abs(actual - complex(re, im)), 1e-10)
        # Merely dropping the neutral's row/column loses its return path.
        zr, yr = reduced("ABCN")
        self.assertGreater(np.max(abs(zr - z[:3, :3])), 0.1)
        np.testing.assert_allclose(yr, y[:3, :3], rtol=0, atol=0)

    def test_native_readback_defects_are_detected_instead_of_relaxing_tolerances(self):
        physical, _ = reference("ABC", 60, False)
        native, _ = reference("ABC", 60, False, native_readback=True)
        comparison = compare(physical, native)
        self.assertFalse(comparison["passed"])
        self.assertEqual(len(comparison["failures"]), 1)
        self.assertTrue(comparison["failures"][0].startswith("line.loss:"))
        physical, _ = reference("BCN", 60, True)
        native, _ = reference("BCN", 60, True, native_readback=True)
        comparison = compare(physical, native)
        self.assertFalse(comparison["passed"])
        self.assertTrue(any("line.from.B.current" in f for f in comparison["failures"]))
        self.assertFalse(any("voltage" in f for f in comparison["failures"]))


if __name__ == "__main__":
    unittest.main()

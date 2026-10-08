"""Independent checks of the source oracle's physical and typed references."""

import unittest

import numpy as np
from dss_source_oracle import (
    PATTERNS,
    load_current,
    materialize_probe_sources,
    parameters,
    reference,
    typed_linear_snapshot,
)


class SourceOracleTests(unittest.TestCase):
    def test_load_laws_scale_and_follow_voltage_angle(self):
        nominal = np.array([240.0])
        power = np.array([480 + 120j])
        current = power.conjugate() / nominal
        for law, factor in (("P", 2), ("I", 1), ("Z", 0.5), ("PIZ", 3.5)):
            got = load_current(np.array([120j]), nominal, power, law)
            np.testing.assert_allclose(got, factor * current * 1j, rtol=1e-14)

    def test_linear_reference_matches_full_coupled_equations(self):
        for pattern in PATTERNS:
            e, nominal, power, z = parameters(pattern)
            y = power.conjugate() / nominal**2
            for floating in (False, True):
                rg = 3 if floating else 0
                matrix = np.eye(len(e)) + np.diag(z * y) + rg * np.tile(y, (len(e), 1))
                sink = np.linalg.solve(matrix, e)
                vn = -rg * sum(y * sink)
                actual, _ = reference(pattern, "Z", floating)
                for i in range(len(e)):
                    self.assertLess(
                        abs(complex(*actual[f"sink.{i}.voltage"]["value"]) - sink[i]),
                        1e-9,
                    )
                    self.assertLess(
                        abs(
                            complex(*actual[f"source.{i}.voltage"]["value"]) - e[i] - vn
                        ),
                        1e-9,
                    )

    def test_typed_mna_and_explicit_source_materialization(self):
        def code(name, r, x):
            return {
                "name": name,
                "n_conductors": 1,
                "r_series": [[r]],
                "x_series": [[x]],
                "g_from": [[0]],
                "g_to": [[0]],
                "b_from": [[0]],
                "b_to": [[0]],
            }

        def line(name, start, end, code):
            return {
                "name": name,
                "bus_from": start,
                "bus_to": end,
                "terminal_map_from": ["1"],
                "terminal_map_to": ["1"],
                "linecode": code,
                "length": 1,
            }

        net = {
            "buses": [
                {"id": b, "terminals": ["1", "4"], "grounded": ["4"]}
                for b in ("internal0", "source", "sink")
            ],
            "linecodes": [code("comp", -10, 0), code("wire", 0.3, 0.6)],
            "lines": [
                line("comp", "internal0", "source", "comp"),
                line("wire", "source", "sink", "wire"),
            ],
            "sources": [
                {
                    "name": "source",
                    "bus": "internal0",
                    "terminal_map": ["1", "4"],
                    "v_magnitude": [240, 0],
                    "v_angle": [0, 0],
                    "extras": {
                        "r1": "10",
                        "r0": "10",
                        "x1": "0",
                        "x0": "0",
                        "bus2": "source.0",
                    },
                }
            ],
            "loads": [
                {
                    "bus": "sink",
                    "terminal_map": ["1", "4"],
                    "p_nom": [480],
                    "q_nom": [120],
                    "voltage_model": {"model": "constant_impedance", "v_nom": [240]},
                }
            ],
            **{
                name: []
                for name in (
                    "switches",
                    "transformers",
                    "generators",
                    "shunts",
                    "untyped",
                )
            },
        }
        document = {"value": {"type": "powerio.MulticonductorNetwork", "data": net}}
        original, _ = typed_linear_snapshot(document, 1, False)
        zl = 240**2 / complex(480, -120)
        self.assertLess(
            abs(
                complex(*original["sink.0.voltage"]["value"])
                - 240 * zl / (zl + 0.3 + 0.6j - 10)
            ),
            1e-9,
        )
        lowered = materialize_probe_sources(document)
        repaired, _ = typed_linear_snapshot(lowered, 1, False)
        self.assertLess(
            abs(
                complex(*repaired["sink.0.voltage"]["value"])
                - 240 * zl / (zl + 0.3 + 0.6j)
            ),
            1e-9,
        )
        self.assertEqual(
            len(net["lines"]), 2, "the original failing IR must remain unchanged"
        )
        self.assertEqual(len(lowered["value"]["data"]["lines"]), 3)


if __name__ == "__main__":
    unittest.main()

"""Independent checks of the source oracle's physical and typed references."""

import unittest

import numpy as np
from dss_source_oracle import (
    PATTERNS,
    load_current,
    materialize_probe_sources,
    parameters,
    reference,
    typed_linear_solution,
    typed_linear_snapshot,
)


class SourceOracleTests(unittest.TestCase):
    def test_coupled_typed_source_matches_loop_equations_with_separate_returns(self):
        emf = np.array([240, 230j])
        z = np.array([[0.3 + 0.7j, 0.2 - 0.1j], [0.1 + 0.05j, 0.6 + 0.9j]])
        ground = np.array([7.0, 3.0])
        nominal = np.array([240.0, 230.0])
        power = np.array([480 + 120j, 350 - 30j])
        load_z = nominal**2 / power.conjugate()
        expected = np.linalg.solve(z + np.diag(ground + load_z), emf)

        def code(name, impedance):
            zero = np.zeros(impedance.shape).tolist()
            return dict(
                name=name,
                n_conductors=len(impedance),
                r_series=impedance.real.tolist(),
                x_series=impedance.imag.tolist(),
                g_from=zero,
                g_to=zero,
                b_from=zero,
                b_to=zero,
            )

        net = {
            "buses": [
                {"id": "feed", "terminals": ["1", "2", "0"], "grounded": ["0"]},
                {
                    "id": "return",
                    "terminals": ["1", "2", "3", "4", "0"],
                    "grounded": ["0"],
                },
            ],
            "linecodes": [
                code("source", z),
                code("ground", np.diag(ground).astype(complex)),
            ],
            "lines": [
                dict(
                    name="source",
                    bus_from="return",
                    bus_to="feed",
                    terminal_map_from=["3", "4"],
                    terminal_map_to=["1", "2"],
                    linecode="source",
                    length=1,
                ),
                dict(
                    name="ground",
                    bus_from="return",
                    bus_to="return",
                    terminal_map_from=["1", "2"],
                    terminal_map_to=["0", "0"],
                    linecode="ground",
                    length=1,
                ),
            ],
            "sources": [
                {
                    "type": "powerio.ReferencedVoltageSource",
                    "value": {
                        "bus": "return",
                        "terminal_map": [str(i + 3)],
                        "reference_terminal": str(i + 1),
                        "v_magnitude": [abs(e)],
                        "v_angle": [np.angle(e)],
                    },
                }
                for i, e in enumerate(emf)
            ],
            "loads": [
                dict(
                    bus="feed",
                    terminal_map=[str(i + 1), "0"],
                    p_nom=[s.real],
                    q_nom=[s.imag],
                    voltage_model=dict(model="constant_impedance", v_nom=[nominal[i]]),
                )
                for i, s in enumerate(power)
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
        volts, currents, _ = typed_linear_solution(document)
        np.testing.assert_allclose(currents["source"], expected, rtol=1e-12)
        np.testing.assert_allclose(currents["ground"], -expected, rtol=1e-12)
        # Source definitions may survive reimport without affecting the typed
        # circuit. An active unconverted element must still fail this oracle.
        for record in (
            {"class": "linecode", "props": [["rmatrix", "[999]"]]},
            {"class": "line", "props": [["enabled", "no"]]},
        ):
            net["untyped"] = [record]
            _, retained_currents, _ = typed_linear_solution(document)
            np.testing.assert_allclose(
                retained_currents["source"], expected, rtol=1e-12
            )
        net["untyped"] = [{"class": "line", "props": [["enabled", "yes"]]}]
        with self.assertRaises(AssertionError):
            typed_linear_solution(document)
        net["untyped"] = []
        for i in range(2):
            self.assertLess(
                abs(volts[("feed", str(i + 1))] - load_z[i] * expected[i]), 1e-10
            )
            self.assertLess(
                abs(volts[("return", str(i + 1))] + ground[i] * expected[i]), 1e-10
            )

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

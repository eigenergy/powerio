"""Audit GLM geometry -> canonical DSS against the original fixed-source circuit.

Failures remain failures, including source defaults and phase-angle changes.
The observed source and line matrices are retained to locate cross-format gaps.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import opendssdirect as dss
from oracle import LOCK, add, command, compare
from overhead_geometry_oracle import deck, reference, MILE


def snapshot(path, flags):
    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    assert LOCK["dss"]["version"] in engine.Basic.Version()
    engine.Text.Command(f'redirect "{path.resolve()}"')
    engine.Text.Command("set controlmode=off tolerance=1e-12 maxiterations=100")
    engine.Text.Command("solve")
    if not engine.Solution.Converged():
        raise ValueError("DSS did not converge")

    def complex_array(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    phases = flags.replace("N", "")
    result = {}
    for bus in ("source", "sink"):
        engine.Circuit.SetActiveBus(bus)
        volts = dict(
            zip(engine.Bus.Nodes(), complex_array(engine.Bus.Voltages()), strict=True)
        )
        for p in phases:
            add(result, f"{bus}.{p}.voltage", "V", volts[ord(p) - 64])
    engine.Circuit.SetActiveElement("line.line")
    n = len(phases)
    assert engine.CktElement.NumConductors() == n
    assert engine.CktElement.NodeOrder() == [ord(p) - 64 for p in phases] * 2
    currents = complex_array(engine.CktElement.Currents())
    powers = complex_array(engine.CktElement.Powers()) * 1000
    for side, label in enumerate(("from", "to")):
        for i, p in enumerate(phases):
            add(result, f"line.{label}.{p}.current", "A", currents[side * n + i])
            add(result, f"line.{label}.{p}.power", "VA", powers[side * n + i])
    add(result, "line.loss", "VA", complex(*engine.CktElement.Losses()))
    engine.LineCodes.Name("line__glm_linecode")
    z = (
        np.array(engine.LineCodes.Rmatrix()) + 1j * np.array(engine.LineCodes.Xmatrix())
    ).reshape(n, n) * MILE
    y = (
        1j
        * 2
        * np.pi
        * engine.Solution.Frequency()
        * 1e-9
        * np.array(engine.LineCodes.Cmatrix()).reshape(n, n)
        * MILE
    )
    for i, p in enumerate(phases):
        for j, q in enumerate(phases):
            add(result, f"line.z.{p}{q}", "ohm", z[i, j])
            add(result, f"line.y.{p}{q}", "S", y[i, j])
    source_settings = {}
    for prop in ("r1", "x1", "r0", "x0", "phases", "angle", "basekv"):
        engine.Text.Command("? vsource.source." + prop)
        source_settings[prop] = engine.Text.Result()
    return result, engine.Basic.Version(), source_settings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--powerio", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    cases = [(flags, 60) for flags in ("A", "AN", "BC", "BCN", "ABC", "ABCN")] + [
        ("ABCN", 50)
    ]
    for flags, f in cases:
        for cap in (False, True):
            key = f"{flags}/{f}/cap_{cap}"
            directory = args.output / key
            directory.mkdir(parents=True)
            source = directory / "input.glm"
            source.write_text(deck(flags, f, cap, "NR"))
            target = directory / "case.dss"
            result = {
                "passed": False,
                "input_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            }
            try:
                result["conversion"] = command(
                    [
                        str(args.powerio.resolve()),
                        "convert",
                        str(source.resolve()),
                        "--to",
                        "opendss",
                        "-o",
                        str(target.resolve()),
                    ],
                    directory,
                )
                result["output_sha256"] = hashlib.sha256(
                    target.read_bytes()
                ).hexdigest()
                actual, version, source_settings = snapshot(target, flags)
                result["version"] = version
                result["source_settings"] = source_settings
                result["observed"] = actual
                expected, _ = reference(flags, f, cap)
                result["comparison"] = compare(expected, actual)
                matrix_keys = [
                    k for k in expected if k.startswith(("line.z.", "line.y."))
                ]
                result["matrix_comparison"] = compare(
                    {k: expected[k] for k in matrix_keys},
                    {k: actual[k] for k in matrix_keys},
                )
                result["passed"] = result["comparison"]["passed"]
            except Exception as error:
                result["error"] = str(error)
            report["cases"][key] = result
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
            print(key, result["passed"], flush=True)
    return 0 if all(v["passed"] for v in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())

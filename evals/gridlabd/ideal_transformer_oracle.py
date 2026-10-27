"""Analytic qualification of emitted ideal coils with grounded/floating returns.

Exercise public DSS parsing, IR restoration, emission and a second import/write.
The original zero-leakage transformer is not runnable directly in OpenDSS.
The independent reference uses the measured primary voltage, ideal ratio and
three known resistances; it checks displacement, physical port currents/powers
and total losses of the emitted transformer-plus-line circuit.
"""

import argparse
import hashlib
import json
from pathlib import Path

import opendssdirect as dss
from oracle import LOCK, add, command, compare


def deck(floating, primary_tap, secondary_tap):
    ret = 2 if floating else 0
    return f"""clear
new circuit.probe phases=1 bus1=source.1.0 basekv=2.4 pu=1 r1=.001 x1=0 r0=.001 x0=0
new transformer.x phases=1 windings=2 buses=(source.1.0 sink.1.{ret}) conns=(wye wye) kvs=(2.4 .24) kvas=(10 10) %rs=(0 0) xscarray=(0) ppm_antifloat=0 taps=({primary_tap} {secondary_tap})
new load.a phases=1 bus1=sink.1.0 kv=.24 kw=1.92 kvar=0 model=2 vminpu=0 vmaxpu=10
new load.c phases=1 bus1=sink.1.{ret} kv=.24 kw=1.92 kvar=0 model=2 vminpu=0 vmaxpu=10
""" + (
        "new load.b phases=1 bus1=sink.2.0 kv=.24 kw=.96 kvar=0 model=2 vminpu=0 vmaxpu=10\n"
        if floating
        else ""
    )


def snapshot(path, floating, ratio, defect=False):
    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("OpenDSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    if defect:
        matrix = "(-.4 | 0 -.4)" if floating else "(-.8)"
        e.Text.Command(f"edit line._pio_xfmr_x rmatrix={matrix}")
    e.Text.Command("set tolerance=1e-12 maxiterations=100 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged():
        raise ValueError("OpenDSS did not converge")

    def complex_values(values):
        return [complex(*values[i : i + 2]) for i in range(0, len(values), 2)]

    def voltages(bus):
        e.Circuit.SetActiveBus(bus)
        return dict(zip(e.Bus.Nodes(), complex_values(e.Bus.Voltages()), strict=True))

    vp = voltages("source")[1]
    vs = vp * ratio
    expected, actual = {}, {}
    v1, v2 = (vs / 3, -2 * vs / 3) if floating else (vs, 0j)
    current = vs / 30 + v1 / 30
    for key, unit, value in (
        ("hot", "V", v1),
        ("return", "V", v2),
        ("from.current", "A", ratio * current),
        ("to.current", "A", -current),
        ("from.power", "VA", vp * (ratio * current).conjugate()),
        ("to.power", "VA", -vs * current.conjugate()),
        ("loss", "VA", 0j),
    ):
        add(expected, key, unit, value)
    v = voltages("sink")
    add(actual, "hot", "V", v[1])
    add(actual, "return", "V", v.get(2, 0j))
    e.Circuit.SetActiveElement("transformer.x")
    add(actual, "from.current", "A", complex_values(e.CktElement.Currents())[0])
    add(actual, "from.power", "VA", 1000 * complex_values(e.CktElement.Powers())[0])
    loss = complex(*e.CktElement.Losses())
    e.Circuit.SetActiveElement("line._pio_xfmr_x")
    n = 2 if floating else 1
    add(actual, "to.current", "A", complex_values(e.CktElement.Currents())[n])
    add(actual, "to.power", "VA", 1000 * sum(complex_values(e.CktElement.Powers())[n:]))
    add(actual, "loss", "VA", loss + complex(*e.CktElement.Losses()))
    return expected, actual


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--powerio", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "engines": LOCK,
        "powerio_sha256": hashlib.sha256(args.powerio.read_bytes()).hexdigest(),
        "cases": {},
    }
    for floating in (False, True):
        for primary_tap, secondary_tap in ((1.0, 1.0), (0.98, 1.03)):
            key = f"{floating}-{primary_tap}-{secondary_tap}"
            directory = args.output / key
            directory.mkdir()
            source = directory / "input.dss"
            source.write_text(deck(floating, primary_tap, secondary_tap))
            checks = {}
            for stage in ("canonical", "reimported"):
                ir, output = directory / f"{stage}.json", directory / f"{stage}.dss"
                command(
                    [str(args.powerio), "serialize", str(source), "-o", str(ir)],
                    directory,
                )
                command(
                    [
                        str(args.powerio),
                        "convert",
                        str(ir),
                        "--to",
                        "opendss",
                        "-o",
                        str(output),
                    ],
                    directory,
                )
                expected, actual = snapshot(
                    output, floating, 0.1 * secondary_tap / primary_tap
                )
                checks[stage] = compare(expected, actual)
                negative = compare(
                    *snapshot(output, floating, 0.1 * secondary_tap / primary_tap, True)
                )
                checks[f"{stage}_wrong_resistance"] = {
                    "passed": not negative["passed"],
                    "mismatch": negative,
                }
                source = output
            item = {
                "input_sha256": hashlib.sha256(
                    (directory / "input.dss").read_bytes()
                ).hexdigest(),
                "checks": checks,
                "passed": all(c["passed"] for c in checks.values()),
            }
            report["cases"][key] = item
            print(key, item["passed"], flush=True)
    report["passed"] = all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

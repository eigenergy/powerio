"""PNNL CONSTANT_PQ generators through import, IR and OpenDSS emission.

Independent diagonal-line circuits cover signed unequal phase powers, native
rating clipping/defaults, scalar initialization overrides and low/high voltages.
The pinned Arras diesel class has different semantics and is not substituted.
"""

import argparse
import cmath
import hashlib
import json
import math
import subprocess
from pathlib import Path

from oracle import LOCK, add, compare, literal, recorded, run_glm

CASES = {
    "unbalanced": ([2000 + 500j, 1000 - 200j, -500 + 100j], 9000, "", 0.02 + 0.04j),
    "parent_alias": ([2000 + 500j, 1000 - 200j, -500 + 100j], 9000, "", 0.02 + 0.04j),
    "capped": ([5000 + 2000j, -4000 - 1000j, 4000j], 9000, "", 0.02 + 0.04j),
    "overrides": (
        [2000 + 500j, 1000 - 200j, -500 + 100j],
        9000,
        "real_power_out_A 2500; reactive_power_out_B 800; real_power_out_C -1;",
        0.02 + 0.04j,
    ),
    "default_rating": ([200000 + 100000j, 1000 - 200j, 0j], 0, "", 0.02 + 0.04j),
    "high_voltage": ([10000 + 2000j, 5000 + 1000j, 2000 - 500j], 90000, "", 2 + 1j),
    "low_voltage": ([-5000 + 0j, -4000 + 0j, -3000 + 0j], 90000, "", 2 + 1j),
}
SOURCE = [277.1281292110204 * cmath.exp(-2j * math.pi * k / 3) for k in range(3)]


def powers(case):
    values, rating, overrides, _ = CASES[case]
    values = list(values)
    if overrides:
        values[0], values[1] = 2500 + 500j, 1000 + 800j
    limit = (rating if rating > 0 else 500000) / 3
    return [p * min(1, limit / abs(p)) if p else 0j for p in values]


def deck(case, solver):
    values, rating, overrides, z = CASES[case]
    text = (
        f"""clock {{ timezone UTC0; timestamp '2001-01-01 0:00:00'; stoptime '2001-01-01 0:00:01'; }}
#set complex_format=%.17g%+.17gj
#set complex_output_format=RECT
module powerflow {{ solver_method {solver}; }}
module generators;
module tape;
object node {{ name source; phases ABCN; bustype SWING; nominal_voltage 277.1281292110204; maximum_voltage_error 1e-10;
"""
        + "\n".join(
            f"voltage_{p} {literal(v)};" for p, v in zip("ABC", SOURCE, strict=True)
        )
        + "\n}\n"
    )
    text += (
        f"""object meter {{ name sink; phases ABCN; nominal_voltage 277.1281292110204; maximum_voltage_error 1e-10; }}
object line_configuration {{ name lc; z11 {literal(z)}; z22 {literal(z)}; z33 {literal(z)}; }}
object overhead_line {{ name line; from source; to sink; phases ABCN; configuration lc; length 1 mile; }}
object diesel_dg {{ name dg; parent sink; Gen_type CONSTANT_PQ; Rated_V 480; Rated_VA {rating}; latitude 0; longitude 151;
"""
        + "\n".join(
            f"power_out_{p} {literal(v)};" for p, v in zip("ABC", values, strict=True)
        )
        + f"\n{overrides}\n}}\n"
    )
    for obj, props in [
        ("sink", [f"voltage_{p}" for p in "ABC"]),
        ("line", [f"current_in_{p}" for p in "ABC"]),
        ("dg", [f"power_out_{p}" for p in "ABC"]),
    ]:
        text += f'object recorder {{ parent {obj}; property {",".join(props)}; interval 1; file "{obj}.csv"; }}\n'
    if case == "parent_alias":
        text = text.replace("parent sink; Gen_type", "parent alias; Gen_type")
        text += "object meter { name alias; parent sink; phases ABCN; nominal_voltage 277.1281292110204; }\n"
    return text.replace(";", ";\n")


def observations(v, i, s):
    result = {}
    for phase, voltage, current, power in zip("ABC", v, i, s, strict=True):
        for label, unit, value in [
            ("voltage", "V", voltage),
            ("current", "A", current),
            ("power", "VA", power),
        ]:
            add(result, phase + "." + label, unit, value)
    return result


def analytic(case):
    s, z, v = powers(case), CASES[case][3], list(SOURCE)
    for _ in range(1000):
        new = [
            e + z * (p / a).conjugate() for e, p, a in zip(SOURCE, s, v, strict=True)
        ]
        if max(abs(a - b) for a, b in zip(v, new, strict=True)) < 1e-12:
            return observations(
                new, [-(p / a).conjugate() for p, a in zip(s, new, strict=True)], s
            )
        v = new
    raise ValueError("analytic fixed point did not converge")


def imported(path, wrong_power=False):
    import opendssdirect as dss

    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("OpenDSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    if wrong_power:
        e.Text.Command("edit generator.dg__A kw=1 kvar=0")
    e.Text.Command("set tolerance=1e-12 maxiterations=1000 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged():
        raise ValueError("OpenDSS did not converge")

    def unpack(raw):
        return [complex(*raw[i : i + 2]) for i in range(0, len(raw), 2)]

    e.Circuit.SetActiveBus("sink")
    bus = dict(zip(e.Bus.Nodes(), unpack(e.Bus.Voltages()), strict=True))
    e.Circuit.SetActiveElement("line.line")
    current = unpack(e.CktElement.Currents())[:3]
    power = []
    for phase in "ABC":
        if e.Circuit.SetActiveElement("generator.dg__" + phase) < 0:
            raise ValueError("missing phase generator")
        power.append(-1000 * unpack(e.CktElement.Powers())[0])
    return observations([bus[i] for i in (1, 2, 3)], current, power)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("bridge", "output", "pnnl-root"):
        p.add_argument("--" + arg, required=True, type=Path)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for case in CASES:
        for solver in ("NR", "FBS"):
            directory = args.output / case / solver
            directory.mkdir(parents=True)
            source = directory / "model.glm"
            source.write_text(deck(case, solver))
            run_glm(
                "pnnl",
                directory,
                args.pnnl_root,
                outputs=("sink.csv", "line.csv", "dg.csv"),
            )
            native = observations(
                *[
                    [
                        recorded(directory / f"{obj}.csv")[f"{prop}_{phase}"]
                        for phase in "ABC"
                    ]
                    for obj, prop in [
                        ("sink", "voltage"),
                        ("line", "current_in"),
                        ("dg", "power_out"),
                    ]
                ]
            )
            proc = subprocess.run(
                [
                    str(args.bridge),
                    str(source),
                    str(directory / "network.json"),
                    "pnnl",
                    "--dss-output",
                    str(directory / "dss"),
                ],
                capture_output=True,
                text=True,
                check=False,
                timeout=120,
            )
            (directory / "bridge.json").write_text(proc.stdout)
            (directory / "bridge.stderr").write_text(proc.stderr)
            proc.check_returncode()
            if not json.loads(proc.stdout)["ir_network_equal"]:
                raise ValueError("IR changed the network")
            actual = imported(directory / "dss/case.dss")
            negative = compare(native, imported(directory / "dss/case.dss", True))
            checks = {
                "analytic_vs_native": compare(analytic(case), native),
                "native_vs_import": compare(native, actual),
                "wrong_power_negative": {
                    "passed": not negative["passed"],
                    "mismatch": negative,
                },
            }
            item = {
                "checks": checks,
                "passed": all(c["passed"] for c in checks.values()),
            }
            report["cases"][f"{case}/{solver}"] = item
            print(case, solver, {k: v["passed"] for k, v in checks.items()}, flush=True)
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
    report["passed"] = all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

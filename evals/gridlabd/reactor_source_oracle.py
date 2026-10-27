"""Static reactor and sequence-source import against both pinned native forks.

Analytic diagonal circuits qualify native equations, actual PowerIO import,
IR and OpenDSS emission. Cases cover native NR zero impedance substitution,
FBS ideal/mixed contacts, phase subsets, sequence rotation and IEEE9500 values.
"""

import argparse
import cmath
import hashlib
import json
import subprocess
from pathlib import Path

from oracle import LOCK, add, compare, literal, recorded, run_glm

CASES = {
    "unbalanced": (
        "ABC",
        [3 + 4j, 240 + 10j, 5 - 2j],
        0,
        [0.1 + 0.2j, 0.3 + 0.4j, 0.5 + 0.6j],
    ),
    "reference-b": (
        "ABC",
        [3 + 4j, 240 + 10j, 5 - 2j],
        1,
        [0.1 + 0.2j, 0.3 + 0.4j, 0.5 + 0.6j],
    ),
    "reference-c": (
        "ABC",
        [3 + 4j, 240 + 10j, 5 - 2j],
        -1,
        [0.1 + 0.2j, 0.3 + 0.4j, 0.5 + 0.6j],
    ),
    "mixed-zero": ("ABC", [0j, 240 + 0j, 0j], 0, [0j, 0.3 + 0.4j, 0j]),
    "all-zero": ("ABC", [0j, 240 + 0j, 0j], 0, [0j, 0j, 0j]),
    "single-a": ("A", [0j, 240 + 0j, 0j], 0, [0.1 + 0.2j, 0j, 0j]),
    "pair-bc": ("BC", [0j, 240 + 0j, 0j], 0, [0j, 0.3 + 0.4j, 0.5 + 0.6j]),
    "ieee9500": ("ABC", [0j, 69715.045 + 0j, 0j], 0, [14.7983j] * 3),
}


def parameters(case, solver):
    phases, seq, rotate, impedances = CASES[case]
    a = cmath.exp(2j * cmath.pi / 3)
    source = [
        seq[0] + seq[1] + seq[2],
        seq[0] + a.conjugate() * seq[1] + a * seq[2],
        seq[0] + a * seq[1] + a.conjugate() * seq[2],
    ]
    source = [v * a**rotate for v in source]
    z = [(1 / (10000 + 10000j) if solver == "NR" and v == 0 else v) for v in impedances]
    load = (
        [10000 + 3000j, 12000 + 2000j, 15000 + 5000j]
        if case == "ieee9500"
        else [30 + 10j, 40 + 5j, 50 + 20j]
    )
    return phases, source, z, load


def deck(case, solver):
    phases, source, _, load = parameters(case, solver)
    _, seq, rotate, z = CASES[case]
    nominal = 66395.28 if case == "ieee9500" else 240
    source_type = "substation" if phases == "ABC" else "node"
    text = f"""clock {{ timezone UTC0; timestamp '2001-01-01 0:00:00'; stoptime '2001-01-01 0:00:01'; }}
#set complex_format=%.17g%+.17gj
#set complex_output_format=RECT
module powerflow {{ solver_method {solver}; }}
module tape;
object {source_type} {{ name source; phases {phases}N; bustype SWING; nominal_voltage {nominal}; maximum_voltage_error 1e-10;
"""
    if source_type == "substation":
        text += f"base_power 12MVA; power_convergence_value 100VA; reference_phase PHASE_{ {0: 'A', 1: 'B', -1: 'C'}[rotate] }; voltage_A 999+1j;\n"
        text += "\n".join(
            f"{name}_sequence_voltage {literal(v)};"
            for name, v in zip(["zero", "positive", "negative"], seq, strict=True)
        )
    else:
        text += "\n".join(
            f"voltage_{p} {literal(source['ABC'.index(p)])};" for p in phases
        )
    text += f"\n}}\nobject load {{ name sink; phases {phases}N; nominal_voltage {nominal}; maximum_voltage_error 1e-10;\n"
    text += "\n".join(
        f"constant_impedance_{p} {literal(load['ABC'.index(p)])};" for p in phases
    )
    text += f"\n}}\nobject series_reactor {{ name reactor; from source; to sink; phases {phases}; latitude 0; longitude 151;\n"
    text += "\n".join(
        f"phase_{p}_impedance {literal(z['ABC'.index(p)])};" for p in phases
    )
    text += "\n}\n"
    for obj, properties in [
        ("source", [f"voltage_{p}" for p in phases]),
        ("sink", [f"voltage_{p}" for p in phases]),
        ("reactor", [f"current_{end}_{p}" for end in ("in", "out") for p in phases]),
    ]:
        text += f'object recorder {{ parent {obj}; property {",".join(properties)}; interval 1; file "{obj}.csv"; }}\n'
    return text.replace(";", ";\n")


def snapshot(phases, source, sink, incoming, outgoing):
    out = {}
    for p in phases:
        for label, unit, value in [
            ("from.voltage", "V", source[p]),
            ("to.voltage", "V", sink[p]),
            ("from.current", "A", incoming[p]),
            ("to.current", "A", outgoing[p]),
            ("from.power", "VA", source[p] * incoming[p].conjugate()),
            ("to.power", "VA", sink[p] * outgoing[p].conjugate()),
        ]:
            add(out, p + "." + label, unit, value)
    return out


def analytic(case, solver):
    phases, e, z, load = parameters(case, solver)
    source = {p: e["ABC".index(p)] for p in phases}
    current = {
        p: e["ABC".index(p)] / (z["ABC".index(p)] + load["ABC".index(p)])
        for p in phases
    }
    sink = {p: current[p] * load["ABC".index(p)] for p in phases}
    return snapshot(phases, source, sink, current, {p: -v for p, v in current.items()})


def native(directory, phases):
    vs = recorded(directory / "source.csv")
    vt = recorded(directory / "sink.csv")
    current = recorded(directory / "reactor.csv")
    return snapshot(
        phases,
        {p: vs["voltage_" + p] for p in phases},
        {p: vt["voltage_" + p] for p in phases},
        {p: current["current_in_" + p] for p in phases},
        {p: -current["current_out_" + p] for p in phases},
    )


def imported(path, phases, negative=False):
    import opendssdirect as dss

    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("DSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    if negative:
        e.Loads.First()
        e.Loads.kW(e.Loads.kW() * 1.1)
    e.Text.Command("set tolerance=1e-12 maxiterations=1000 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged():
        raise ValueError("DSS did not converge")

    def complex_values(v):
        return [complex(*v[i : i + 2]) for i in range(0, len(v), 2)]

    def volts(bus):
        e.Circuit.SetActiveBus(bus)
        return dict(zip(e.Bus.Nodes(), complex_values(e.Bus.Voltages()), strict=True))

    vs, vt = volts("source"), volts("sink")
    current = {p: 0j for p in phases}
    for name in e.Loads.AllNames():
        e.Circuit.SetActiveElement("load." + name)
        if e.CktElement.BusNames()[0].split(".")[0] != "sink":
            raise ValueError("unexpected load bus")
        for node, value in zip(
            e.CktElement.NodeOrder(),
            complex_values(e.CktElement.Currents()),
            strict=True,
        ):
            if node in (1, 2, 3):
                current["ABC"[node - 1]] += value
    return snapshot(
        phases,
        {p: vs["ABC".index(p) + 1] for p in phases},
        {p: vt["ABC".index(p) + 1] for p in phases},
        current,
        {p: -v for p, v in current.items()},
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("bridge", "pnnl-root", "output"):
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for case in CASES:
        for solver in ("NR", "FBS"):
            for profile in ("pnnl", "arras"):
                directory = args.output / case / solver / profile
                directory.mkdir(parents=True)
                source = directory / "model.glm"
                source.write_text(deck(case, solver))
                run_glm(
                    profile,
                    directory,
                    args.pnnl_root,
                    outputs=("source.csv", "sink.csv", "reactor.csv"),
                )
                proc = subprocess.run(
                    [
                        str(args.bridge),
                        str(source),
                        str(directory / "network.json"),
                        profile,
                        "--dss-output",
                        str(directory / "dss"),
                    ],
                    capture_output=True,
                    text=True,
                    timeout=120,
                    check=False,
                )
                (directory / "bridge.json").write_text(proc.stdout)
                (directory / "bridge.stderr").write_text(proc.stderr)
                proc.check_returncode()
                if not json.loads(proc.stdout)["ir_network_equal"]:
                    raise ValueError("IR mismatch")
                phases = CASES[case][0]
                expected = analytic(case, solver)
                actual = native(directory, phases)
                negative = compare(
                    expected, imported(directory / "dss/case.dss", phases, True)
                )
                checks = {
                    "analytic_vs_native": compare(expected, actual),
                    "native_vs_import": compare(
                        actual, imported(directory / "dss/case.dss", phases)
                    ),
                    "wrong_load_negative": {
                        "passed": not negative["passed"],
                        "mismatch": negative,
                    },
                }
                item = {
                    "checks": checks,
                    "passed": all(v["passed"] for v in checks.values()),
                }
                report["cases"][f"{case}/{solver}/{profile}"] = item
                print(
                    case,
                    solver,
                    profile,
                    {k: v["passed"] for k, v in checks.items()},
                    flush=True,
                )
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
    report["passed"] = all(v["passed"] for v in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

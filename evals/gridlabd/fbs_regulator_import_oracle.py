"""FBS fixed regulators through actual PowerIO import, IR and OpenDSS output.

Checks both native forks against ideal ratio equations. OpenDSS uses a finite
transformer leakage plus a cancelling line; physical port powers/losses include
both elements. The typed network retains the ideal zero-leakage constraint.
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from oracle import LOCK, add, compare, run_glm
from regulator_oracle import CASES, deck, expected, instruments, names, read_glm


def imported(path, phases, defect=False):
    import opendssdirect as dss

    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("OpenDSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    mapping = names(phases, True)
    if defect:
        name = next(iter(mapping.values()))
        phase = "ABC".index(phases[0]) + 1
        e.Text.Command(f"edit line._pio_xfmr_{name} enabled=no")
        e.Text.Command(f"edit transformer.{name} wdg=2 bus=sink.{phase}.0")
    e.Text.Command("set tolerance=1e-12 maxiterations=100 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged() or e.Solution.ControlMode() != -1:
        raise ValueError("OpenDSS did not converge with controls disabled")

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    result = {}
    for bus in ("source", "sink"):
        e.Circuit.SetActiveBus(bus)
        v = dict(zip(e.Bus.Nodes(), unpack(e.Bus.Voltages()), strict=True))
        for p in phases:
            add(result, f"{bus}.{p}", "V", v["ABC".index(p) + 1])
    loss = 0j
    for index, (p, name) in enumerate(mapping.items()):
        e.Circuit.SetActiveElement(f"transformer.{name}")
        i, s = unpack(e.CktElement.Currents()), 1000 * unpack(e.CktElement.Powers())
        add(result, f"from.{p}.current", "A", i[0])
        add(result, f"from.{p}.power", "VA", s[0])
        loss += complex(*e.CktElement.Losses())
        if defect and index == 0:
            add(result, f"to.{p}.current", "A", i[2])
            add(result, f"to.{p}.power", "VA", s[2])
        else:
            e.Circuit.SetActiveElement(f"line._pio_xfmr_{name}")
            i, s = unpack(e.CktElement.Currents()), 1000 * unpack(e.CktElement.Powers())
            add(result, f"to.{p}.current", "A", i[1])
            add(result, f"to.{p}.power", "VA", s[1])
            loss += complex(*e.CktElement.Losses())
    add(result, "loss", "VA", loss)
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--bridge", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--pnnl-root", type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for label, case in list(CASES.items()):
        kind, phases, taps, _, override = case
        # Native FBS ignores regulator_resistance even when explicitly nonzero.
        CASES[label] = (kind, phases, taps, 0.0, override)
        text = (
            deck(label)
            .replace("solver_method NR", "solver_method FBS")
            .replace("regulator_resistance 0.0;", "regulator_resistance 7.3;")
        )
        recorders, outputs = instruments(phases, False)
        text += recorders
        for profile in ("pnnl", "arras"):
            directory = args.output / label / profile
            directory.mkdir(parents=True)
            source = directory / "model.glm"
            source.write_text(text)
            command = [
                str(args.bridge),
                str(source),
                str(directory / "network.pio.json"),
                profile,
                "--dss-output",
                str(directory / "dss"),
            ]
            proc = subprocess.run(
                command, capture_output=True, text=True, timeout=60, check=False
            )
            (directory / "bridge.stdout").write_text(proc.stdout)
            (directory / "bridge.stderr").write_text(proc.stderr)
            proc.check_returncode()
            data = json.loads(proc.stdout)
            if not data["ir_network_equal"]:
                raise ValueError("IR changed network")
            run_glm(profile, directory, args.pnnl_root, outputs=outputs)
            native = read_glm(directory, phases, False)
            actual = imported(directory / "dss/case.dss", phases)
            analytic = expected(label)
            checks = {
                "native_vs_import": compare(native, actual),
                "analytic_vs_native": compare(analytic, native),
                "analytic_vs_import": compare(analytic, actual),
            }
            negative = compare(
                native, imported(directory / "dss/case.dss", phases, True)
            )
            if negative["passed"]:
                raise ValueError("uncancelled leakage unexpectedly matched")
            checks["uncancelled_leakage_negative"] = {
                "passed": True,
                "mismatch": negative,
            }
            item = {
                "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
                "checks": checks,
                "passed": all(c["passed"] for c in checks.values()),
            }
            report["cases"][f"{label}/{profile}"] = item
            (directory / "measurements.json").write_text(
                json.dumps(
                    {"native": native, "imported": actual, "analytic": analytic},
                    indent=2,
                )
                + "\n"
            )
            print(
                label,
                profile,
                item["passed"],
                checks["native_vs_import"]["failures"][:2],
                flush=True,
            )
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
    report["passed"] = all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

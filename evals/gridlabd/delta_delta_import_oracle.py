"""Native FBS delta-delta hybrid law, IR and exact OpenDSS external ports."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from delta_grounded_import_oracle import cases as grounded_cases
from delta_grounded_import_oracle import native_snapshot, snapshot
from oracle import LOCK, compare, run_glm


def cases():
    for label, text, params in grounded_cases():
        if not label.endswith("FBS"):
            continue
        for delta in (False, True):
            source = text.replace("DELTA_GWYE", "DELTA_DELTA")
            if delta:
                source = source.replace(
                    "name sink; phases ABCN", "name sink; phases ABCD"
                )
            yield label + ("-delta" if delta else "-wye"), source, (*params, delta)


def analytic(params):
    vp, vs, kva, zpu, source, load, delta = params
    ratio = vp / vs
    impedance = zpu * vs**2 / (kva * 1000)
    projection = np.eye(3) - np.ones((3, 3)) / 3
    transfer = np.array([[1, 0, 0], [0, 1, 0], [-1, -1, 0]])
    incidence = np.array([[1, -1, 0], [0, 1, -1], [-1, 0, 1]]) if delta else np.eye(3)
    admittance = incidence.T @ np.diag(1 / load) @ incidence
    volts = np.linalg.solve(
        np.eye(3) + impedance * transfer @ admittance, projection @ source / ratio
    )
    outgoing = admittance @ volts
    incoming = outgoing / ratio
    sp, st = source * np.conj(incoming), -volts * np.conj(outgoing)
    return snapshot(source, volts, incoming, -outgoing, sp, st, sum(sp + st))


def imported(path, wrong=False):
    import opendssdirect as dss

    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("DSS pin mismatch")
    engine.Text.Command(f'redirect "{path}"')
    if wrong:
        engine.Text.Command("edit line.line__glm_to r1=1.1 r0=1.1")
    engine.Text.Command("set controlmode=off tolerance=1e-12 maxiterations=100")
    engine.Solution.Solve()
    if not engine.Solution.Converged() or engine.Solution.ControlMode() != -1:
        raise ValueError("DSS did not converge with controls disabled")

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    volts, currents, powers = [], [], []
    for bus, side in (("source", "from"), ("sink", "to")):
        if engine.Circuit.SetActiveBus(bus) < 0:
            raise ValueError("missing external bus")
        values = dict(
            zip(engine.Bus.Nodes(), unpack(engine.Bus.Voltages()), strict=True)
        )
        volts.append(np.array([values[p] for p in (1, 2, 3)]))
        if engine.Circuit.SetActiveElement(f"line.line__glm_{side}") < 0:
            raise ValueError("missing external contact")
        if engine.CktElement.NodeOrder()[:3] != [1, 2, 3]:
            raise ValueError("unexpected external conductor ordering")
        currents.append(unpack(engine.CktElement.Currents())[:3])
        powers.append(1000 * unpack(engine.CktElement.Powers())[:3])
    return snapshot(*volts, *currents, *powers, sum(powers[0] + powers[1]))


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
    for label, text, params in cases():
        expected = analytic(params)
        for profile in ("pnnl", "arras"):
            d = args.output / label / profile
            d.mkdir(parents=True)
            source = d / "model.glm"
            source.write_text(text)
            run_glm(
                profile,
                d,
                args.pnnl_root,
                outputs=("source.csv", "sink.csv", "line.csv", "impedance.xml"),
            )
            native = native_snapshot(d)
            proc = subprocess.run(
                [
                    str(args.bridge),
                    str(source),
                    str(d / "network.json"),
                    profile,
                    "--dss-output",
                    str(d / "dss"),
                ],
                capture_output=True,
                text=True,
                timeout=120,
                check=False,
            )
            (d / "bridge.json").write_text(proc.stdout)
            (d / "bridge.stderr").write_text(proc.stderr)
            proc.check_returncode()
            if not json.loads(proc.stdout)["ir_network_equal"]:
                raise ValueError("IR changed network")
            actual = imported(d / "dss/case.dss")
            negative = compare(native, imported(d / "dss/case.dss", True))
            checks = {
                "native_vs_analytic": compare(expected, native),
                "native_vs_import": compare(native, actual),
                "wrong_impedance_negative": {
                    "passed": not negative["passed"],
                    "mismatch": negative,
                },
            }
            report["cases"][label + "/" + profile] = {
                "checks": checks,
                "passed": all(c["passed"] for c in checks.values()),
            }
            report["passed"] = all(c["passed"] for c in report["cases"].values())
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
            print(
                label,
                profile,
                {k: v["passed"] for k, v in checks.items()},
                checks["native_vs_import"]["failures"][:2],
                flush=True,
            )
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

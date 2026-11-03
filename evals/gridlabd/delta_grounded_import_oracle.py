"""Delta-grounded-wye import through IR to OpenDSS, with physical port checks.

Synthetic step-down, step-up and equal-rating cases exercise both native phase
orientations, NR/FBS, unbalanced sources/loads and primary zero-sequence blocking.
No external feeder contents are vendored. The IEEE8500-parameters case uses its
substation nameplate values, not the complete feeder.
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from oracle import LOCK, add, compare, literal, recorded, run_glm


def cases():
    for label, vp, vs, kva, zpu, solver in [
        (f"{ratio}-{solver}", vp, vs, 500, 0.011 + 0.02j, solver)
        for ratio, vp, vs in [
            ("down", 4160, 480),
            ("up", 480, 4160),
            ("equal", 480, 480),
        ]
        for solver in ("NR", "FBS")
    ] + [("ieee8500-parameters", 115000, 12470, 27500, 0.01344 + 0.1551j, "FBS")]:
        primary = vp / np.sqrt(3)
        source = (
            primary
            * np.array([1.02, 0.97, 1.01])
            * np.exp(1j * np.deg2rad([2, -119, 121]))
        )
        source += primary * (0.025 + 0.01j)  # common mode must not reach secondary
        load = (
            vs**2
            / 3
            / np.conj(np.array([0.2 + 0.07j, 0.12 + 0.03j, 0.1 + 0.05j]) * kva * 1000)
        )
        text = [
            "#set complex_format=%.15g%+.15gj",
            "#set complex_output_format=RECT",
            "clock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:02'; }",
            f"module powerflow {{ solver_method {solver}; }}",
            "module tape;",
            f"object node {{ name source; phases ABCN; bustype SWING; nominal_voltage {primary:.17g}; maximum_voltage_error 1e-10;",
            " ".join(
                f"voltage_{p} {literal(v)};" for p, v in zip("ABC", source, strict=True)
            )
            + " }",
            f"object load {{ name sink; phases ABCN; nominal_voltage {vs / np.sqrt(3):.17g}; maximum_voltage_error 1e-10;",
            " ".join(
                f"constant_impedance_{p} {literal(z)};"
                for p, z in zip("ABC", load, strict=True)
            )
            + " }",
            f"object transformer_configuration {{ name cfg; connect_type DELTA_GWYE; install_type PADMOUNT; primary_voltage {vp}; secondary_voltage {vs}; power_rating {kva}; impedance {literal(zpu)}; }}",
            "object transformer { name line; phases ABCN; from source; to sink; configuration cfg; }",
            'object impedance_dump { filename "impedance.xml"; }',
        ]
        for bus in ("source", "sink"):
            text.append(
                f'object recorder {{ parent {bus}; property voltage_A,voltage_B,voltage_C; interval 1; file "{bus}.csv"; }}'
            )
        fields = [
            f"{q}_{end}_{p}"
            for end in ("in", "out")
            for p in "ABC"
            for q in ("current", "power")
        ] + ["power_losses"]
        text.append(
            f'object recorder {{ parent line; property {",".join(fields)}; interval 1; file "line.csv"; }}'
        )
        yield label, "\n".join(text) + "\n", (vp, vs, kva, zpu, source, load)


def snapshot(vp, vs, ip, it, sp, st, loss):
    result = {}
    for bus, volts, current, power in (("source", vp, ip, sp), ("sink", vs, it, st)):
        for p, v, i, s in zip("ABC", volts, current, power, strict=True):
            add(result, f"{bus}.{p}.voltage", "V", v)
            add(result, f"{bus}.{p}.current", "A", i)
            add(result, f"{bus}.{p}.power", "VA", s)
    add(result, "transformer.loss", "VA", loss)
    return result


def native_snapshot(path):
    rows = [recorded(path / f"{b}.csv") for b in ("source", "sink")]
    vp, vs = [np.array([row[f"voltage_{p}"] for p in "ABC"]) for row in rows]
    line = recorded(path / "line.csv")
    ip, it = [
        np.array([sign * line[f"current_{end}_{p}"] for p in "ABC"])
        for end, sign in (("in", 1), ("out", -1))
    ]
    sp, st = [
        np.array([sign * line[f"power_{end}_{p}"] for p in "ABC"])
        for end, sign in (("in", 1), ("out", -1))
    ]
    return snapshot(vp, vs, ip, it, sp, st, line["power_losses"])


def imported_snapshot(path, defect=False):
    import opendssdirect as dss

    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("OpenDSS version pin mismatch")
    engine.Text.Command(f'redirect "{path}"')
    if defect:
        engine.Text.Command("edit transformer.line__glm_A wdg=1 bus=source.1.0")
    engine.Text.Command("set tolerance=1e-12 maxiterations=100 controlmode=off")
    engine.Solution.Solve()
    if not engine.Solution.Converged() or engine.Solution.ControlMode() != -1:
        raise ValueError("OpenDSS did not converge with controls disabled")

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    volts = []
    for bus in ("source", "sink"):
        engine.Circuit.SetActiveBus(bus)
        values = dict(
            zip(engine.Bus.Nodes(), unpack(engine.Bus.Voltages()), strict=True)
        )
        volts.append(np.array([values[k] for k in (1, 2, 3)]))
    currents = np.zeros((2, 3), dtype=complex)
    powers = currents.copy()
    loss = 0j
    for phase in "ABC":
        if engine.Circuit.SetActiveElement(f"transformer.line__glm_{phase}") < 0:
            raise ValueError("missing mapped coil")
        nodes = engine.CktElement.NodeOrder()
        i, p = (
            unpack(engine.CktElement.Currents()),
            1000 * unpack(engine.CktElement.Powers()),
        )
        if len(nodes) != 4:
            raise ValueError("unexpected transformer conductor count")
        for k, node in enumerate(nodes):
            if node:
                currents[k // 2, node - 1] += i[k]
                powers[k // 2, node - 1] += p[k]
        loss += complex(*engine.CktElement.Losses())
    return snapshot(*volts, *currents, *powers, loss)


def analytic(params):
    vp, vs, kva, zpu, source, load = params
    # Three independent ideal primary coils. Solve each secondary's series
    # impedance and load, then scatter its reflected current to primary nodes.
    other = (2, 0, 1) if vp > vs else (1, 2, 0)
    ratio = vp / (vs / np.sqrt(3))
    z = zpu * vs**2 / (kva * 1000)
    current = np.array(
        [(source[k] - source[j]) / ratio / (z + load[k]) for k, j in enumerate(other)]
    )
    sink = load * current
    ip = np.zeros(3, dtype=complex)
    for k, j in enumerate(other):
        ip[k] += current[k] / ratio
        ip[j] -= current[k] / ratio
    it = -current
    sp, st = source * np.conj(ip), sink * np.conj(it)
    return snapshot(source, sink, ip, it, sp, st, sum(abs(current) ** 2 * z))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for label, text, params in cases():
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
            if not json.loads(proc.stdout)["ir_network_equal"]:
                raise ValueError("IR changed network")
            run_glm(profile, directory, args.pnnl_root)
            native = native_snapshot(directory)
            actual = imported_snapshot(directory / "dss/case.dss")
            expected = analytic(params)
            checks = {
                "native_vs_import": compare(native, actual),
                "analytic_vs_native": compare(expected, native),
                "analytic_vs_import": compare(expected, actual),
            }
            if label == "down-NR":
                negative = compare(
                    native, imported_snapshot(directory / "dss/case.dss", True)
                )
                if negative["passed"]:
                    raise ValueError(
                        "grounded-primary negative control unexpectedly matched"
                    )
                checks["grounded_primary_negative"] = {
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
                    {"native": native, "imported": actual, "analytic": expected},
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

"""Analytic, native and imported FBS lines with noncommuting Z/Y matrices."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from ieee8500_import_oracle import grouped
from ieee9500_import_oracle import (
    PORT_PROPERTIES,
    imported_snapshot,
    inventory,
    native_snapshot,
)
from oracle import LOCK, add, compare, run_glm


def cases():
    for phases in ("ABC", "AC"):
        for length in (0.2, 1.0):
            n = len(phases)
            z = np.full((n, n), 0.07 + 0.12j)
            np.fill_diagonal(z, [1 + 0.5j, 0.8 + 0.6j, 1.2 + 0.4j][:n])
            cap = np.full((n, n), -12000.0)
            np.fill_diagonal(cap, [100000, 80000, 60000][:n])
            source = np.array([240, -116 - 205j, -124 + 210j])[
                ["ABC".index(p) for p in phases]
            ]
            load = np.array([20 + 5j, 35 + 12j, 15 + 3j])[:n]
            y = 1j * 2 * np.pi * 60 * cap * 1e-9 * length
            zz = z * length
            a = np.eye(n) + zz @ y / 2
            c = y + y @ zz @ y / 4
            sink = np.linalg.solve(a + zz @ np.diag(1 / load), source)
            outgoing = sink / load
            incoming = c @ sink + a @ outgoing
            expected = {}
            for bus, volts in (("source", source), ("sink", sink)):
                for phase in "ABC":
                    add(
                        expected,
                        f"bus.{bus}.{phase}",
                        "V",
                        volts[phases.index(phase)] if phase in phases else 0j,
                    )
            loss = 0j
            for side, volts, amps in (
                ("in", source, incoming),
                ("out", sink, -outgoing),
            ):
                powers = volts * np.conj(amps)
                loss += sum(powers)
                for phase in "ABC":
                    for quantity, unit, vals in (
                        ("current", "A", amps),
                        ("power", "VA", powers),
                    ):
                        add(
                            expected,
                            f"link.line.{side}.{phase}.{quantity}",
                            unit,
                            vals[phases.index(phase)] if phase in phases else 0j,
                        )
            add(expected, "link.line.loss", "VA", loss)

            def value(v):
                return f"{v.real:.17g}{v.imag:+.17g}j"

            fields = " ".join(
                f"z{'ABC'.index(p) + 1}{'ABC'.index(q) + 1} {value(z[i, j])}; c{'ABC'.index(p) + 1}{'ABC'.index(q) + 1} {cap[i, j]:.17g};"
                for i, p in enumerate(phases)
                for j, q in enumerate(phases)
            )
            text = f"""clock {{ timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:01'; }}
module powerflow {{ solver_method FBS; line_capacitance TRUE; default_maximum_voltage_error 1e-12; }}
module tape;
object node {{ name source; phases {phases}N; bustype SWING; nominal_voltage 240; {" ".join(f"voltage_{p} {value(source[i])};" for i, p in enumerate(phases))} }}
object load {{ name sink; phases {phases}N; nominal_voltage 240; {" ".join(f"constant_impedance_{p} {value(load[i])};" for i, p in enumerate(phases))} }}
object line_configuration {{ name cfg; {fields} }}
object overhead_line {{ name line; phases {phases}N; from source; to sink; configuration cfg; length {length * 5280:.17g}; }}
#set complex_format=%.17g%+.17gj
#set complex_output_format=RECT
"""
            yield f"{phases}-{length}", text, expected


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("bridge", "pnnl-root", "output"):
        p.add_argument("--" + arg, type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for label, text, expected in cases():
        objects = inventory(text)
        fields = [
            f"{name}:{prop}"
            for name, obj in objects.items()
            for prop in (
                PORT_PROPERTIES
                if obj["class"] == "overhead_line"
                else [f"voltage_{p}" for p in "ABC"]
            )
        ]
        chunks = [fields[i : i + 12] for i in range(0, len(fields), 12)]
        for profile in ("pnnl", "arras"):
            d = args.output / label / profile
            d.mkdir(parents=True)
            (d / "records").mkdir()
            model = text + "\n".join(
                f'object multi_recorder {{ property {",".join(chunk)}; file "records/chunk{i}.csv"; interval 1; }}'
                for i, chunk in enumerate(chunks)
            )
            (d / "model.glm").write_text(model)
            run_glm(profile, d, args.pnnl_root, outputs=("records/.",))
            native, _ = native_snapshot(grouped(d, {"chunks": chunks}), objects)
            proc = subprocess.run(
                [
                    str(args.bridge),
                    str(d / "model.glm"),
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
            bridge = json.loads(proc.stdout)
            if not bridge["ir_network_equal"]:
                raise ValueError("IR mismatch")
            actual, _ = imported_snapshot(
                d / "dss/case.dss", bridge["network"], objects
            )
            wrong, _ = imported_snapshot(
                d / "dss/case.dss", bridge["network"], objects, wrong_tap=True
            )
            bad = compare(expected, wrong)
            item = {
                "analytic_native": compare(expected, native),
                "analytic_import": compare(expected, actual),
                "wrong_contact_negative": {
                    "passed": not bad["passed"],
                    "mismatch": bad,
                },
            }
            item["passed"] = all(v["passed"] for v in item.values())
            report["cases"][f"{label}-{profile}"] = item
            report["passed"] = all(v["passed"] for v in report["cases"].values())
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
            print(
                label,
                profile,
                item["passed"],
                item["analytic_native"]["failures"][:2],
                item["analytic_import"]["failures"][:2],
                flush=True,
            )
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

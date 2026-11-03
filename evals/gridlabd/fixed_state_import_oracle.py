"""Parent capacitor voltage, single-phase banks, and connected SWING_PQ states."""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

from ieee8500_import_oracle import grouped
from ieee9500_import_oracle import (
    imported_snapshot,
    instrument,
    inventory,
    native_snapshot,
)
from oracle import LOCK, compare, run_glm
from reactor_source_oracle import deck


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
    for case in ("parent-rating", "explicit-rating", "connected-swing-pq"):
        for profile in ("pnnl",) if case == "connected-swing-pq" else ("pnnl", "arras"):
            d = args.output / case / profile
            d.mkdir(parents=True)
            text = re.sub(r"object recorder\s*\{[^{}]*\}", "", deck("unbalanced", "NR"))
            text += "\nobject capacitor { name cap; parent sink; phases BN; nominal_voltage 120; control MANUAL; control_level BANK; pt_phase B; capacitor_B 1440; switchB CLOSED; "
            if case == "explicit-rating":
                text += "cap_nominal_voltage 120; "
            text += "}\n"
            if case == "connected-swing-pq":
                text = text.replace("name sink;", "name sink; bustype SWING_PQ;")
                text += """module reliability { report_event_log false; }
object fault_check { name check; check_mode ONCHANGE; strictly_radial false; grid_association true; eventgen_object events; }
object eventgen { name events; fault_type DLG-X; manual_outages "reactor,2100-01-01 00:00:05,2100-01-01 00:00:30"; }
"""
            text = text.replace(";", ";\n")
            source = d / "input.glm"
            source.write_text(text)
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
            b = json.loads(proc.stdout)
            if not b["ir_network_equal"]:
                raise ValueError("IR mismatch")
            objects = inventory(text)
            model, spec = instrument(text, objects)
            if profile == "arras":
                # Small bounded multi-recorder avoids the pinned Arras group
                # recorder's complex-output truncation.
                model = re.sub(r"object group_recorder\s*\{[^{}]*\}", "", model)
                fields = [
                    f"{name}:{prop}"
                    for name, obj in objects.items()
                    for prop in spec[obj["class"]]
                ]
                chunks = [fields[i : i + 12] for i in range(0, len(fields), 12)]
                (d / "records").mkdir()
                for i, fields in enumerate(chunks):
                    model += f'object multi_recorder {{ property {",".join(fields)}; file "records/chunk{i}.csv"; interval 1; }}\n'
                spec = {"chunks": chunks}
            (d / "model.glm").write_text(model)
            outputs = (
                ("records/.",)
                if profile == "arras"
                else tuple(
                    f"{cls}-{prop}.csv" for cls, props in spec.items() for prop in props
                )
            )
            run_glm(profile, d, args.pnnl_root, outputs=outputs)
            expected, _ = native_snapshot(grouped(d, spec), objects)
            actual, _ = imported_snapshot(d / "dss/case.dss", b["network"], objects)
            wrong = d / "dss/wrong.dss"
            wrong.write_text(
                (d / "dss/case.dss").read_text() + "\nEdit Line.reactor length=2\n"
            )
            changed, _ = imported_snapshot(wrong, b["network"], objects)
            check = compare(expected, actual)
            negative = compare(expected, changed)
            item = {
                "comparison": check,
                "wrong_impedance_negative": {
                    "passed": not negative["passed"],
                    "mismatch": negative,
                },
                "passed": check["passed"] and not negative["passed"],
            }
            report["cases"][case + "/" + profile] = item
            report["passed"] = all(v["passed"] for v in report["cases"].values())
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
            print(case, profile, item["passed"], check["failures"][:3], flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

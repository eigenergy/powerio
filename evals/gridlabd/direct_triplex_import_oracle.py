"""Direct triplex matrices: native hot-port law, IR and exact DSS equivalents.

The native zero neutral-current telemetry is not a physical neutral model.
"""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

from oracle import LOCK, add, compare, recorded, run_glm
from triplex_import_oracle import OUTPUTS, cases, imported_snapshot, native_snapshot


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
    _, phase, original = next(cases())
    for label, sign in (("ieee9500", 1), ("reciprocal-physical", -1)):
        text = re.sub(
            r"object triplex_line_configuration\s*\{[^{}]*\}",
            f"object triplex_line_configuration {{ name cable; z11 2.16454+.880800j; z12 .623542+.673688j; z21 {sign * 0.623542}{sign * 0.673688:+}j; z22 {sign * 2.16454}{sign * 0.880800:+}j; }}",
            original,
        )
        for solver in ("NR", "FBS"):
            for profile in ("pnnl", "arras"):
                d = args.output / label / solver / profile
                d.mkdir(parents=True)
                source = d / "model.glm"
                source.write_text(
                    re.sub(r"solver_method (NR|FBS)", "solver_method " + solver, text)
                )
                run_glm(profile, d, args.pnnl_root, outputs=OUTPUTS)
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
                bridge = json.loads(proc.stdout)
                if not bridge["ir_network_equal"]:
                    raise ValueError("IR mismatch")
                native = native_snapshot(d, phase)
                service = recorded(d / "service.csv")
                # Native power_losses sums absolute per-phase real losses.
                # Direct matrices may have negative physical phase losses;
                # compare signed physical input minus output at both ports.
                physical_loss = sum(
                    service[f"power_in_{p}"] - service[f"power_out_{p}"] for p in "AB"
                )
                del native["service.loss"]
                add(native, "service.loss", "VA", physical_loss)
                actual = imported_snapshot(d / "dss/case.dss", phase, direct=True)
                # Neither source dataset nor native direct matrix specifies N current.
                # Zero telemetry stays outside the physical acceptance comparison.
                for snap in (native, actual):
                    for key in list(snap):
                        if key.startswith("service.") and ".N." in key:
                            del snap[key]
                check = compare(native, actual)
                wrong = d / "dss/wrong.dss"
                wrong.write_text(
                    (d / "dss/case.dss").read_text()
                    + "\nEdit Load.sink__glm_impedance_1 kw=2 kvar=1\n"
                )
                negative = imported_snapshot(wrong, phase, direct=True)
                for key in list(negative):
                    if key.startswith("service.") and ".N." in key:
                        del negative[key]
                bad = compare(native, negative)
                item = {
                    "native_reported_loss": [
                        service["power_losses"].real,
                        service["power_losses"].imag,
                    ],
                    "native_physical_loss": [physical_loss.real, physical_loss.imag],
                    "comparison": check,
                    "wrong_load_negative": {
                        "passed": not bad["passed"],
                        "mismatch": bad,
                    },
                    "passed": check["passed"] and not bad["passed"],
                }
                report["cases"][label + "/" + solver + "/" + profile] = item
                report["passed"] = all(v["passed"] for v in report["cases"].values())
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(
                    label,
                    solver,
                    profile,
                    item["passed"],
                    check["failures"][:2],
                    flush=True,
                )
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

"""Fresh and edited reactor/substation GLM output from qualified imported IR.

Uses the reference-B sequence circuit and IEEE9500 source/reactor parameters.
Both native forks solve canonical output; canonical reimport passes through IR
and actual OpenDSS emission again. Edits rotate source angles and scale reactor
impedance in the public IR document, with independent changed analytic results.
"""

import argparse
import cmath
import hashlib
import json
import re
import subprocess
from pathlib import Path

from oracle import LOCK, compare, run_glm
from reactor_source_oracle import imported, native, parameters, snapshot


def expected(case, edited):
    phases, source, z, load = parameters(case, "NR")
    if edited:
        source = [v * cmath.exp(0.07j) for v in source]
        z = [v * 1.1 for v in z]
    vs = {p: source["ABC".index(p)] for p in phases}
    current = {p: vs[p] / (z["ABC".index(p)] + load["ABC".index(p)]) for p in phases}
    vt = {p: current[p] * load["ABC".index(p)] for p in phases}
    return snapshot(phases, vs, vt, current, {p: -v for p, v in current.items()})


def command(argv, directory, label):
    proc = subprocess.run(
        [str(x) for x in argv], capture_output=True, text=True, timeout=120, check=False
    )
    (directory / (label + ".stdout")).write_text(proc.stdout)
    (directory / (label + ".stderr")).write_text(proc.stderr)
    proc.check_returncode()
    return proc.stdout


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("powerio", "bridge", "pnnl-root", "import-results", "output"):
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    original_report = args.import_results / "report.json"
    if not json.loads(original_report.read_text())["passed"]:
        raise ValueError("original imports must qualify first")
    report = {
        "engines": LOCK,
        "input_report_sha256": hashlib.sha256(original_report.read_bytes()).hexdigest(),
        "binary_sha256": {
            name: hashlib.sha256(getattr(args, name).read_bytes()).hexdigest()
            for name in ("bridge", "powerio")
        },
        "cases": {},
    }
    for case in ("reference-b", "ieee9500"):
        for profile in ("pnnl", "arras"):
            original = args.import_results / case / "NR" / profile
            source_text = (original / "model.glm").read_text()
            recorders = "\n".join(
                re.findall(r"object recorder\s*\{[^}]+\}", source_text)
            )
            prefix = source_text.split("module powerflow", 1)[0] + "module tape;\n"
            for edited in (False, True):
                directory = (
                    args.output / case / profile / ("edited" if edited else "fresh")
                )
                directory.mkdir(parents=True)
                ir = json.loads((original / "network.json").read_text())
                if edited:
                    net = ir["value"]["data"]["value"]["network"]
                    if len(net["sources"]) != 1 or len(net["linecodes"]) != 1:
                        raise ValueError("unexpected circuit")
                    net["sources"][0]["v_angle"] = [
                        v + 0.07 for v in net["sources"][0]["v_angle"]
                    ]
                    for field in ("r_series", "x_series"):
                        net["linecodes"][0][field] = [
                            [1.1 * v for v in row] for row in net["linecodes"][0][field]
                        ]
                input_ir = directory / "input.json"
                input_ir.write_text(json.dumps(ir, indent=2) + "\n")
                target = "gridlabd" if profile == "pnnl" else "gridlabd-arras"
                command(
                    [
                        args.powerio,
                        "convert",
                        input_ir,
                        "--to",
                        target,
                        "-o",
                        directory / "canonical",
                    ],
                    directory,
                    "emit",
                )
                text = (directory / "canonical/case.glm").read_text()
                (directory / "model.glm").write_text(
                    prefix + text + "\n" + recorders + "\n"
                )
                run_glm(
                    profile,
                    directory,
                    args.pnnl_root,
                    outputs=("source.csv", "sink.csv", "reactor.csv"),
                )
                value = json.loads(
                    command(
                        [
                            args.bridge,
                            directory / "model.glm",
                            directory / "restored.json",
                            profile,
                            "--dss-output",
                            directory / "dss",
                        ],
                        directory,
                        "reimport",
                    )
                )
                if not value["ir_network_equal"]:
                    raise ValueError("IR mismatch")
                actual = native(directory, "ABC")
                ref = expected(case, edited)
                checks = {
                    "analytic_vs_canonical": compare(ref, actual),
                    "canonical_vs_reimport": compare(
                        actual, imported(directory / "dss/case.dss", "ABC")
                    ),
                }
                if edited:
                    negative = compare(expected(case, False), actual)
                    checks["unedited_negative"] = {
                        "passed": not negative["passed"],
                        "mismatch": negative,
                    }
                item = {
                    "checks": checks,
                    "passed": all(v["passed"] for v in checks.values()),
                }
                report["cases"][f"{case}/{profile}/{edited}"] = item
                print(
                    case,
                    profile,
                    edited,
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

"""Qualify the staged exact source writer with P/I/Z/PIZ loaded circuits.

The native original first matches an independent analytic reference. Canonical
output and a second canonical output after staged typed reimport then match all
original line/source port voltages, currents and powers. No ordinary API
activation or general nonlinear typed solver qualification is claimed.
"""

import argparse
import hashlib
import json
from pathlib import Path

from dss_source_lowering_oracle import canonical_snapshot, native
from dss_source_oracle import LAWS, PATTERNS, deck, native_snapshot, reference
from oracle import compare


def prepare(output):
    output.mkdir(parents=True, exist_ok=False)
    inputs = {}
    for pattern in PATTERNS:
        for law in ("Z",) if pattern == "zero" else LAWS:
            for floating in (False, True):
                for frequency in (50, 60):
                    for resistance in (1.0, 10.0, 100.0):
                        key = f"{pattern}/{law}/{floating}/{frequency}/{resistance}"
                        path = output / key / "case.dss"
                        path.parent.mkdir(parents=True)
                        path.write_text(
                            deck(pattern, law, floating, frequency, resistance)
                        )
                        inputs[key] = str(path.resolve())
    (output / "inputs.json").write_text(json.dumps(inputs, indent=2) + "\n")
    print(f"Prepared {len(inputs)} circuits")


def evaluate(output, canonical):
    inputs = json.loads((output / "inputs.json").read_text())
    report = {"scope": __doc__, "cases": {}}
    for key, path in inputs.items():
        pattern, law, floating, _, _ = key.split("/")
        floating = floating == "True"
        result = {
            "passed": False,
            "input_sha256": hashlib.sha256(Path(path).read_bytes()).hexdigest(),
        }
        try:
            expected, convergence = reference(pattern, law, floating)
            observed, version = native_snapshot(
                Path(path), len(PATTERNS[pattern]), floating
            )
            analytic = compare(expected, observed)
            ports, elements, _ = native(Path(path))
            checks = {}
            for stage, filename in [
                ("canonical", "case.dss"),
                ("reimported", "case.reimport.dss"),
            ]:
                written = canonical / key / filename
                actual = canonical_snapshot(written, elements)
                checks[stage] = compare(ports, actual)
                result[f"{stage}_sha256"] = hashlib.sha256(
                    written.read_bytes()
                ).hexdigest()
            result.update(
                version=version,
                convergence=convergence,
                analytic=analytic,
                checks=checks,
                passed=analytic["passed"] and all(v["passed"] for v in checks.values()),
            )
        except Exception as error:
            result["error"] = f"{type(error).__name__}: {error}"
        report["cases"][key] = result
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    passed = sum(v["passed"] for v in report["cases"].values())
    print(f"{passed}/{len(inputs)} loaded source circuits passed")
    return int(passed != len(inputs))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "evaluate"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--canonical", type=Path)
    args = parser.parse_args()
    if args.action == "evaluate" and args.canonical is None:
        parser.error("evaluate requires --canonical")
    raise SystemExit(
        prepare(args.output)
        if args.action == "prepare"
        else evaluate(args.output, args.canonical)
    )

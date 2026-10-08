#!/usr/bin/env python3
"""Run the native 12-bus public reader, electrical oracle and negative controls.

Requires NumPy, OpenDSSDirect.py and a built sincal_native_multiconductor
example. The unlicensed native model must remain outside the repository.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "reader", "report"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    checker = Path(__file__).with_name("check_truong12.py").resolve()
    with tempfile.TemporaryDirectory(prefix="powerio-truong12-") as directory:
        root = Path(directory)
        network = root / "network.json"
        with network.open("w") as output:
            run = subprocess.run([str(args.reader.resolve()), str(args.source.resolve())],
                                 stdout=output, stderr=subprocess.PIPE, text=True,
                                 check=True, timeout=120)
        consumers = json.loads(run.stderr)
        if consumers != {"source_echo": True, "typed_ir": True,
                         "generic_matrix_diagnostics": 0, "power_flow_instance": True}:
            raise ValueError("public consumer checks differ")
        command = [sys.executable, str(checker), str(args.source.resolve())]
        passed = root / "passed.json"
        subprocess.run([*command, str(network), str(passed)],
                       capture_output=True, text=True, check=True, timeout=120)
        report = json.loads(passed.read_text())
        original = json.loads(network.read_text())
        controls = []
        for name in ("missing-load", "wrong-line-impedance", "wrong-load-phase"):
            changed = copy.deepcopy(original)
            if name == "missing-load":
                changed["loads"].pop()
            elif name == "wrong-line-impedance":
                for field in ("r_series", "x_series"):
                    changed["linecodes"][0][field] = [
                        [value * 2 for value in row]
                        for row in changed["linecodes"][0][field]]
            else:
                switch = next(s for s in changed["switches"]
                              if s["bus_to"].startswith("sincal:load:")
                              and s["terminal_map_from"] == ["1"])
                switch["terminal_map_from"] = ["2"]
            mutated = root / (name + ".json")
            mutated.write_text(json.dumps(changed))
            run = subprocess.run([*command, str(mutated), str(root / "invalid.json")],
                                 capture_output=True, text=True, timeout=120)
            expected = ("incomplete Load accounting" if name == "missing-load"
                        else "independent voltage disagreement")
            if run.returncode == 0 or "ValueError: " + expected not in run.stderr:
                raise ValueError(f"negative control {name} did not fail as expected: {run.stderr}")
            controls.append({"mutation": name, "rejected": True, "reason": expected})
        report.update({"reader_api": "public facade, native SQLite",
                       "reader_sha256": hashlib.sha256(args.reader.read_bytes()).hexdigest(),
                       "checker_sha256": hashlib.sha256(checker.read_bytes()).hexdigest(),
                       "generic_consumers": consumers, "negative_controls": controls})
        args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Complete 12-bus asymmetric case passed; {len(controls)} negative controls rejected.")


if __name__ == "__main__":
    main()

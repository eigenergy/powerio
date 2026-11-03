"""Static BatchEdit matching, ordered assignments and electrical comparisons."""

import argparse
import hashlib
import json
from pathlib import Path

from ieee4_import_oracle import evaluate

BASE = """clear
new circuit.c basekv=12.47 pu=.94 r1=.01 x1=.02 r0=.02 x0=.03
new line.line1 phases=3 bus1=sourcebus bus2=end r1=.4 r0=.6 x1=.3 x0=.5 c1=0 c0=0 length=1 units=km
new load.alpha1 phases=1 bus1=end.1.0 kv=7.199557856794634 kw=40 kvar=10
new load.beta2 phases=1 bus1=end.2.0 kv=7.199557856794634 kw=50 kvar=15
new load.gamma3 phases=1 bus1=end.3.0 kv=7.199557856794634 kw=60 kvar=20
new load.last4 phases=1 bus1=end.1.0 kv=7.199557856794634 kw=70 kvar=25
"""
CASES = {
    "all": "batchedit load..* vminpu=.88",
    "substring_case": "batchedit load.ALP vminpu=.88",
    "anchored_alternation": "batchedit load.^(alpha1|BETA2)$ vminpu=.88",
    "character_classes": "batchedit load.^[ab][a-z]+[12]$ vminpu=.88",
    "later_override": "batchedit load..* vminpu=.88\nedit load.beta2 vminpu=.97",
    "active_after_unmatched": "batchedit load.^alpha vminpu=.88\n~ kv=7.8",
    "no_matches": "batchedit load.^absent$ vminpu=.88\n~ kv=7.8",
    "line_state": "batchedit load..* vminpu=.88\nbatchedit line..* r1=.8 r0=1.2 x1=.6 x0=1",
    "copied_properties": "batchedit load.^[ab] like=gamma3\nbatchedit load..* vminpu=.88",
    "repeated_assignment": "batchedit load..* vminpu=.97 vminpu=.88",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(
        scope=__doc__,
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
    )
    for name, edits in CASES.items():
        folder = args.output / name
        folder.mkdir()
        source = folder / "input.dss"
        source.write_text(
            BASE
            + edits
            + "\nset controlmode=off tolerance=1e-11 maxiterations=1000\nsolve\n"
        )
        row = evaluate(source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-11)
        row["input_sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
        report["cases"][name] = row
        print(name, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()

"""Public IEEE13/34/37/123/9500 and North American LV DSS feeders at their written, fixed device states.

Every solve and voltage-base calculation in the selected entry files is
preceded by ControlMode=Off. Original bytes are pinned and retained separately;
this is an explicit static profile, not a claim about active regulator results.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

from ieee4_import_oracle import evaluate
from oracle import HERE

PREFIX = "Version8/Distrib/IEEETestCases/"
CASES = {
    "ieee13": "13Bus/IEEE13Nodeckt.dss",
    "ieee34_mod1": "34Bus/ieee34Mod1.dss",
    "ieee34_mod2": "34Bus/ieee34Mod2.dss",
    "ieee37": "37Bus/ieee37.dss",
    "ieee123": "123Bus/IEEE123Master.dss",
    "ieee123_switches": "123Bus/IEEE123Switches.dss",
    "ieee9500": "ieee9500dss/ieee9500_base.dss",
    "north_american": "LVTestCaseNorthAmerican/Master.dss",
    "north_american_secpar": "LVTestCaseNorthAmerican/SecPar.dss",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source-root",
        type=Path,
        required=True,
        help="pinned electricdss-tst checkout",
    )
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    catalog = json.loads((HERE / "ieee-corpus.json").read_text())
    records = {}
    commits = set()
    for family in catalog["families"]:
        if family["id"] not in (
            "ieee13",
            "ieee34",
            "ieee37",
            "ieee123",
            "north_american_lv_network",
            "ieee9500",
        ):
            continue
        for pin in family["sources"]:
            if not pin["repository"].endswith("/electricdss-tst"):
                continue
            commits.add(pin["commit"])
            for record in pin["files"]:
                if record["path"] in records and records[record["path"]] != record:
                    raise ValueError("inconsistent source pins")
                records[record["path"]] = record
    if len(commits) != 1:
        raise ValueError("expected one pinned source revision")
    original = args.output / "original"
    fixed = args.output / "fixed"
    patches = {}
    hashes = {}
    for relative, record in records.items():
        data = (args.source_root / relative).read_bytes()
        if (
            len(data) != record["bytes"]
            or hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
            != record["git_blob_sha1"]
        ):
            raise ValueError(f"source pin mismatch: {relative}")
        a, b = original / relative, fixed / relative
        a.parent.mkdir(parents=True, exist_ok=True)
        b.parent.mkdir(parents=True, exist_ok=True)
        a.write_bytes(data)
        adjusted = data
        if relative in {PREFIX + value for value in CASES.values()}:
            lines = data.decode("utf-8-sig").splitlines(keepends=True)
            result = []
            edits = []
            for number, line in enumerate(lines, 1):
                code = re.split(r"!|//", line)[0].strip()
                if re.match(r"(?i)^(solve|calcv|calcvoltagebases)(?:\s|$)", code):
                    result.append("Set ControlMode=Off\n")
                    edits.append(
                        dict(
                            before_line=number,
                            original_command=code,
                            insertion="Set ControlMode=Off",
                        )
                    )
                result.append(line)
            if not edits:
                raise ValueError(
                    f"selected feeder has no recognized calculation: {relative}"
                )
            adjusted = "".join(result).encode()
            patches[relative] = edits
        b.write_bytes(adjusted)
        hashes[relative] = dict(
            record,
            original_sha256=hashlib.sha256(data).hexdigest(),
            profile_sha256=hashlib.sha256(adjusted).hexdigest(),
        )
    report = dict(
        scope=__doc__,
        source_commit=commits.pop(),
        files=hashes,
        profile_patches=patches,
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
    )
    for name, relative in CASES.items():
        folder = args.output / name
        folder.mkdir()
        row = evaluate(
            (fixed / PREFIX / relative).resolve(),
            args.bridge.resolve(),
            folder.resolve(),
            1e-11,
            fixed.resolve(),
        )
        row["entry"] = PREFIX + relative
        report["cases"][name] = row
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(name, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
    report["qualified_cases"] = sum(v["passed"] for v in report["cases"].values())
    report["passed"] = all(v["passed"] for v in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Qualified {report['qualified_cases']}/{len(CASES)} fixed-state cases.")


if __name__ == "__main__":
    main()

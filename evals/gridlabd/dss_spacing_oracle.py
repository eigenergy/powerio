"""Check pinned public geometry definitions in controlled static circuits.

This qualifies the selected LineSpacing/LineGeometry definitions, not the full
North American feeder. Direct Line.spacing remains separate compatibility work.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

from ieee4_import_oracle import evaluate
from oracle import HERE


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--north-american-master", required=True, type=Path)
    parser.add_argument("--bridge", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    catalog = json.loads((HERE / "ieee-corpus.json").read_text())
    family = next(
        f for f in catalog["families"] if f["id"] == "north_american_lv_network"
    )
    pin = family["sources"][0]
    record = next(f for f in pin["files"] if f["path"].endswith("/Master.dss"))
    data = args.north_american_master.read_bytes()
    if (
        len(data) != record["bytes"]
        or hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
        != record["git_blob_sha1"]
    ):
        raise ValueError("public input differs from pinned master")
    lines = data.decode("utf-8-sig").splitlines(keepends=True)
    start = next(
        i for i, line in enumerate(lines) if re.match(r"(?i)^new linespacing\.", line)
    )
    end = next(i for i, line in enumerate(lines) if re.match(r"(?i)^new line\.", line))
    definitions = "".join(lines[start:end])
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(
        scope=__doc__,
        source=dict(repository=pin["repository"], commit=pin["commit"], **record),
        selected_source_lines=[start + 1, end],
        definitions_sha256=hashlib.sha256(definitions.encode()).hexdigest(),
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
    )
    for geometry, length in (("Trans", 150), ("Pri", 300), ("Sec", 30)):
        folder = args.output / geometry.lower()
        folder.mkdir()
        source = folder / "input.dss"
        source.write_text(
            "Clear\nNew Circuit.c phases=3 bus1=sourcebus basekv=12.47 pu=1.02 "
            "r1=.01 x1=.02 r0=.02 x0=.03\n"
            + definitions
            + f"New Line.probe bus1=sourcebus.1.2.3 bus2=end.1.2.3 geometry={geometry} "
            f"length={length} units=ft earthmodel=carson\n"
            + "\n".join(
                f"New Load.l{i} phases=1 bus1=end.{i}.0 kv=7.199557856794634 "
                f"kw={kw} kvar={kw / 2} model=1"
                for i, kw in enumerate((80, 120, 65), 1)
            )
            + "\nSet controlmode=off tolerance=1e-11 maxiterations=1000\nSolve\n"
        )
        row = evaluate(source.resolve(), args.bridge.resolve(), folder.resolve(), 1e-11)
        row["profile_sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
        report["cases"][geometry] = row
        print(geometry, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()

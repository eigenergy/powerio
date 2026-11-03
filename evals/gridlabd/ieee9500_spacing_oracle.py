"""Pinned IEEE9500 direct-spacing definitions in controlled unbalanced circuits.

These selected line probes do not qualify the complete feeder.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

from ieee4_import_oracle import evaluate
from oracle import HERE


def blocks(text):
    lines = text.splitlines(keepends=True)
    starts = [i for i, line in enumerate(lines) if re.match(r"(?i)^new\s", line)]
    for a, b in zip(starts, [*starts[1:], len(lines)], strict=True):
        yield a + 1, "".join(lines[a:b])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--master", required=True, type=Path)
    parser.add_argument("--bridge", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    catalog = json.loads((HERE / "ieee-corpus.json").read_text())
    family = next(f for f in catalog["families"] if f["id"] == "ieee9500")
    pin = next(
        s for s in family["sources"] if s["repository"].endswith("/electricdss-tst")
    )
    record = next(f for f in pin["files"] if f["path"].endswith("ieee9500_base.dss"))
    data = args.master.read_bytes()
    if (
        len(data) != record["bytes"]
        or hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
        != record["git_blob_sha1"]
    ):
        raise ValueError("IEEE9500 master differs from source pin")
    source_blocks = list(blocks(data.decode("utf-8-sig")))
    definitions = "".join(
        block
        for _, block in source_blocks
        if re.match(r"(?i)^new (wiredata|cndata|linespacing)\.", block)
    )
    selected = {}
    for line, block in source_blocks:
        if not re.match(r"(?i)^new line\.", block) or not re.search(
            r"\bspacing=", block, re.I
        ):
            continue
        phases = int(re.search(r"(?i)\bphases=(\d+)", block)[1])
        kind = "cn" if re.search(r"(?i)\bcncables=", block) else "overhead"
        selected.setdefault((kind, phases), (line, block))
    if not selected:
        raise ValueError("no direct-spacing lines found")
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(
        scope=__doc__,
        source=dict(repository=pin["repository"], commit=pin["commit"], **record),
        definitions_sha256=hashlib.sha256(definitions.encode()).hexdigest(),
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
    )
    for (kind, phases), (line, original) in selected.items():
        name = f"{kind}-{phases}ph"
        folder = args.output / name
        folder.mkdir()
        circuit_line = re.sub(r"(?i)^(new line\.)\S+", r"\1probe", original)
        nodes = re.search(r"(?i)\bbus1=\S+?\.([\d.]+)", original)[1]
        circuit_line = re.sub(
            r"(?i)\bbus1=\S+", "bus1=sourcebus." + nodes, circuit_line
        )
        circuit_line = re.sub(r"(?i)\bbus2=\S+", "bus2=end." + nodes, circuit_line)
        path = folder / "input.dss"
        path.write_text(
            "clear\nnew circuit.c bus1=sourcebus basekv=12.47 r1=.01 x1=.02 r0=.02 x0=.03\n"
            + definitions
            + circuit_line
            + "\n".join(
                f"new load.l{i} phases=1 bus1=end.{node}.0 kv=7.199557856794634 kw={40 + 15 * i} kvar={20 + 5 * i}"
                for i, node in enumerate(nodes.split("."))
                if node != "0"
            )
            + "\nset controlmode=off tolerance=1e-11 maxiterations=1000\nsolve\n"
        )
        row = evaluate(path.resolve(), args.bridge.resolve(), folder.resolve(), 1e-11)
        row.update(
            source_line=line,
            original_line_block=original,
            profile_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
        )
        report["cases"][name] = row
        print(name, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()

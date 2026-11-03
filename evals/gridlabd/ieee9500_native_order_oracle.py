"""Native-only sensitivity to independent fixed-switch declaration order.

Use the output directory of ieee9500_archive_import_oracle.py as --profile-run.
This diagnoses native reproducibility; it never qualifies PowerIO conversion.
All component port inventories and YPrim entries must remain identical.
"""

import argparse
import hashlib
import json
import re
import shutil
from pathlib import Path

from european_lv_import_oracle import snapshot
from ieee4_import_oracle import run
from oracle import compare

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--profile-run", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
profiles = json.loads((args.profile_run / "report.json").read_text())
out = args.output
out.mkdir(exist_ok=False)
report = {
    "scope": "Native-only independent explicitly parameterized switch Line declaration reordering; no PowerIO conversion or tolerance changes",
    "cases": {},
}
for variant in ["bal", "unbal"]:
    src = args.profile_run / variant / "fixed"
    dst = out / variant
    for member, record in profiles["cases"][variant]["files"].items():
        relative = Path(member)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("unsafe profile member")
        if (
            hashlib.sha256((src / relative).read_bytes()).hexdigest()
            != record["profile_sha256"]
        ):
            raise ValueError("changed profile member")
    shutil.copytree(src, dst)
    path = dst / "dss" / f"ieee9500{variant}_base.dss"
    raw = path.read_text()
    lines = raw.splitlines(keepends=True)
    blocks = []
    i = 0
    while i < len(lines):
        m = re.match(r"(?i)^new\s+line\.([^\s]+)", lines[i])
        if not m or "switch=y" not in lines[i].lower():
            i += 1
            continue
        name = m[1].lower()
        start = i
        i += 1
        while i < len(lines):
            text = re.split("!|//", lines[i])[0].strip()
            if not text or text.startswith("~"):
                i += 1
                continue
            state = re.fullmatch(
                r"(?i)(?:open|close)\s+line\.([^\s]+)\s+\d+(?:\s+\d+)?", text
            )
            if state and state[1].lower() == name:
                i += 1
                continue
            break
        block = "".join(lines[start:i])
        assert not re.search(r"(?i)\blike\s*=", block)
        blocks.append((start, i, block))
    altered = []
    pos = 0
    for (start, end, _), (_, _, text) in zip(blocks, reversed(blocks), strict=True):
        altered.extend(lines[pos:start])
        altered.append(text)
        pos = end
    altered.extend(lines[pos:])
    path.write_text("".join(altered))
    native = run(src / "dss" / path.name, 1e-11)
    other = run(path, 1e-11)
    names = [n.lower() for n in native.Circuit.AllElementNames()]
    assert set(names) == set(n.lower() for n in other.Circuit.AllElementNames())
    max_y = 0.0
    changed = []
    for name in names:
        native.Circuit.SetActiveElement(name)
        other.Circuit.SetActiveElement(name)
        assert [n.lower() for n in native.CktElement.BusNames()] == [
            n.lower() for n in other.CktElement.BusNames()
        ], name
        assert native.CktElement.NodeOrder() == other.CktElement.NodeOrder(), name
        y = native.CktElement.YPrim()
        z = other.CktElement.YPrim()
        assert len(y) == len(z), name
        error = max((abs(a - b) for a, b in zip(y, z, strict=True)), default=0.0)
        max_y = max(max_y, error)
        if error:
            changed.append((name, error))
    buses = [b.lower() for b in native.Circuit.AllBusNames()]
    physical = [
        n
        for n in names
        if n.startswith(
            (
                "line.",
                "transformer.",
                "load.",
                "autotrans.",
                "capacitor.",
                "reactor.",
                "generator.",
                "pvsystem.",
                "storage.",
            )
        )
    ]
    a = snapshot(native, buses, physical, canonical=False)
    b = snapshot(other, buses, physical, canonical=False)
    (dst / "original.json").write_text(json.dumps(a))
    (dst / "reordered.json").write_text(json.dumps(b))
    result = compare(a, b)
    report["cases"][variant] = {
        "engine": native.Basic.Version(),
        "line_blocks": len(blocks),
        "original_sha256": hashlib.sha256(raw.encode()).hexdigest(),
        "reordered_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "max_yprim_scalar_error": max_y,
        "changed_yprim": changed,
        "order_control_valid": not changed,
        "comparison": result,
    }
    print(
        variant,
        "native-vs-native",
        len(result["failures"]),
        "of",
        result["observables"],
        result["max_absolute_error"],
        "YPrim",
        max_y,
        flush=True,
    )
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")

if any(not row["order_control_valid"] for row in report["cases"].values()):
    raise SystemExit(
        "reordering changed native electrical primitives; not a valid numerical control"
    )

#!/usr/bin/env python3
"""Produce every powerio conversion the validation matrix compares against, in one
process (import powerio once instead of per conversion). Writes outputs to <tmp>
by the naming convention validate_oracles.jl expects:

    <stem>.pmjson.json   <m>  -> PowerModels JSON   (PMjson writer leg)
    <stem>.psse.raw      <m>  -> PSS/E .raw         (PSSE writer leg)
    <stem>.reemit.json   <stem>.pmref.json -> PowerModels JSON  (PMread reader leg)
    psse_<stem>.json     <raw> -> PowerModels JSON  (PSSE read side)
    egret_<stem>.json    <egret.json> -> PowerModels JSON (egret read side)

A failed or empty conversion is reported and the output left absent, so the
matching comparison leg fails on a missing file rather than on stale data.

    python benchmarks/convert_cases.py <tmp> --m <m>... --raw <r>... --egret <e>...
"""

import sys
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
import powerio


def convert(inp, to, out):
    try:
        text = powerio.convert_file(str(inp), to).text
    except Exception as exc:  # noqa: BLE001 — report any reader/writer failure, keep going
        print(f"  convert FAIL {Path(inp).name} -> {to}: {exc}", file=sys.stderr)
        return
    if not text:
        print(f"  convert EMPTY {Path(inp).name} -> {to}", file=sys.stderr)
        return
    Path(out).write_text(text)


def main():
    if len(sys.argv) < 2:
        print("usage: convert_cases.py <tmp> --m <m>... --raw <r>... --egret <e>...", file=sys.stderr)
        return 2
    tmp = Path(sys.argv[1])
    groups = {"m": [], "raw": [], "egret": []}
    cur = None
    for arg in sys.argv[2:]:
        if arg.startswith("--"):
            cur = arg[2:]
            if cur not in groups:
                print(f"unknown group flag --{cur}", file=sys.stderr)
                return 2
        elif cur is None:
            print(f"argument before any --group flag: {arg}", file=sys.stderr)
            return 2
        else:
            groups[cur].append(arg)

    for m in groups["m"]:
        stem = Path(m).stem
        convert(m, "powermodels-json", tmp / f"{stem}.pmjson.json")
        convert(m, "psse", tmp / f"{stem}.psse.raw")
        pmref = tmp / f"{stem}.pmref.json"
        if pmref.is_file():
            convert(pmref, "powermodels-json", tmp / f"{stem}.reemit.json")
    for r in groups["raw"]:
        convert(r, "powermodels-json", tmp / f"psse_{Path(r).stem}.json")
    for e in groups["egret"]:
        convert(e, "powermodels-json", tmp / f"egret_{Path(e).stem}.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())

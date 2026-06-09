#!/usr/bin/env python3
"""Regenerate the benchmark speed tables in benchmarks/RESULTS.md
from the JSON the bench scripts emit, so the numbers stop being copy-pasted by hand.

Reads benchmarks/results/{speed_julia,speed_python}.json (written by
`bench_julia.jl --json` and `bench_parse.py --json`) and rewrites only the regions
fenced by `<!-- BENCH:<id> START -->` / `<!-- BENCH:<id> END -->`. Prose outside the
markers is never touched.

Scope: the speed tables only. The correctness matrix and the version block in
RESULTS.md stay hand-written — correctness is a boolean gated in CI (run_validation.sh),
not a table that drifts every run.

    python benchmarks/render_tables.py            # rewrite the tables in place
    python benchmarks/render_tables.py --check    # exit 1 if a table is out of date

A region whose JSON is missing any of its canonical cases is left UNCHANGED with a
warning (run `bash benchmarks/fetch_cases.sh` and re-bench), so a partial run never
silently shrinks a published table. Stdlib only; does not import powerio.
"""

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RESULTS_DIR = REPO / "benchmarks" / "results"

SPEED_HEADER = (
    "| case | buses / branches | powerio | ExaPowerIO.jl | PowerModels.jl |\n"
    "| --- | --- | --- | --- | --- |"
)
PANDA_HEADER = (
    "| case | powerio parse | matpowercaseframes (pandapower's `.m` reader) |\n"
    "| --- | --- | --- |"
)

# Canonical case order per region. A region renders only when its JSON carries
# every case listed here.
SPEED_JULIA_CASES = [
    "case2869pegase", "case_ACTIVSg2000", "case9241pegase", "case13659pegase",
    "case_ACTIVSg10k", "case_ACTIVSg25k", "case_ACTIVSg70k", "case_SyntheticUSA",
    "case99k", "case193k",
]
PANDA_CASES = ["case2869pegase", "case9241pegase", "case13659pegase", "case193k"]


def ms(value):
    return "n/a" if value is None else f"{value} ms"


def _select(rows, cases):
    """Rows for `cases` in order, or (None, missing) when the JSON lacks any of them."""
    by_case = {r["case"]: r for r in rows}
    missing = [c for c in cases if c not in by_case]
    return (None, missing) if missing else ([by_case[c] for c in cases], [])


def julia_rows(rows, cases):
    selected, missing = _select(rows, cases)
    if selected is None:
        return None, missing
    lines = [
        f"| {r['case']} | {r['buses']} / {r['branches']} | "
        f"{ms(r['powerio_ms'])} | {ms(r['exapowerio_ms'])} | {ms(r['powermodels_ms'])} |"
        for r in selected
    ]
    return "\n".join(lines), []


def panda_rows(rows, cases):
    selected, missing = _select(rows, cases)
    if selected is None:
        return None, missing
    lines = [
        f"| {r['case']} | {ms(r['powerio_parse_ms'])} | {ms(r['matpowercaseframes_ms'])} |"
        for r in selected
    ]
    return "\n".join(lines), []


def load(name):
    path = RESULTS_DIR / name
    if not path.exists():
        return None
    return json.loads(path.read_text())


def splice(text, region_id, body):
    pat = re.compile(
        rf"(<!-- BENCH:{re.escape(region_id)} START -->\n).*?(\n<!-- BENCH:{re.escape(region_id)} END -->)",
        re.DOTALL,
    )
    if not pat.search(text):
        raise SystemExit(f"error: marker BENCH:{region_id} not found; refusing to write")
    return pat.sub(lambda m: m.group(1) + body + m.group(2), text, count=1)


def main():
    check = "--check" in sys.argv[1:]
    speed_julia = load("speed_julia.json")
    speed_python = load("speed_python.json")

    # (region id, target file, table body or None, list of missing cases)
    plan = []
    if speed_julia is not None:
        body, missing = julia_rows(speed_julia["rows"], SPEED_JULIA_CASES)
        plan.append(("speed-julia", "benchmarks/RESULTS.md", SPEED_HEADER, body, missing))
    if speed_python is not None:
        body, missing = panda_rows(speed_python["rows"], PANDA_CASES)
        plan.append(("speed-pandapower", "benchmarks/RESULTS.md", PANDA_HEADER, body, missing))

    if not plan:
        raise SystemExit(f"error: no JSON in {RESULTS_DIR} — run the bench scripts with --json first")

    edits = {}  # file -> (original text, edited text); each file is read once
    for region, target, header, body, missing in plan:
        if body is None:
            print(f"skip BENCH:{region}: JSON missing {', '.join(missing)} (fetch + re-bench); left unchanged")
            continue
        if target not in edits:
            text = (REPO / target).read_text()
            edits[target] = (text, text)
        original, current = edits[target]
        edits[target] = (original, splice(current, region, f"{header}\n{body}"))

    changed = []
    for target, (original, new_text) in edits.items():
        if original != new_text:
            changed.append(target)
            if not check:
                (REPO / target).write_text(new_text)

    if check:
        if changed:
            print("out of date: " + ", ".join(changed))
            return 1
        print("benchmark tables up to date")
        return 0
    print("updated: " + (", ".join(changed) if changed else "nothing (already current)"))
    return 0


if __name__ == "__main__":
    sys.exit(main())

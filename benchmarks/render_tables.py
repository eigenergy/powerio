#!/usr/bin/env python3
"""Regenerate the benchmark speed tables in benchmarks/RESULTS.md
from the JSON the bench scripts emit, so the numbers stop being copied by hand.

Reads benchmarks/results/{speed_julia,speed_python,speed_powerworld,speed_matrix}.json
(written by `bench_julia.jl --json`, `bench_parse.py --json`, and the
Criterion extractors documented in RESULTS.md) and rewrites only the regions
fenced by `<!-- BENCH:<id> START -->` / `<!-- BENCH:<id> END -->`.
Prose outside the markers is never touched.

Scope: the speed tables only. The correctness matrix and the version block in
RESULTS.md stay written by hand; correctness is a boolean gated in CI
(run_validation.sh), separate from per run timing noise.

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
    "| case | buses / branches | PowerIO.jl parse_file | ExaPowerIO.jl parse | PowerModels.jl parse | Rust C ABI handle | net.data |\n"
    "| --- | --- | --- | --- | --- | --- | --- |"
)
SPEED_YBUS_HEADER = (
    "| case | buses / branches | PowerIO.jl Ybus | ExaPowerIO.jl Ybus | Rust C ABI Arrow | PowerModels.jl Ybus |\n"
    "| --- | --- | --- | --- | --- | --- |"
)
PANDA_HEADER = (
    "| case | powerio parse | powerio parse + Y_bus + Bp | matpowercaseframes (pandapower's `.m` reader) |\n"
    "| --- | --- | --- | --- |"
)
POWERWORLD_HEADER = (
    "| case | buses / branches | aux | pwb |\n"
    "| --- | --- | --- | --- |"
)
MATRIX_HEADER = (
    "| operation | case | buses / branches | mean |\n"
    "| --- | --- | --- | --- |"
)

# Canonical case order per region. A region renders only when its JSON carries
# every case listed here.
SPEED_JULIA_CASES = [
    "case2869pegase", "case_ACTIVSg2000", "case9241pegase", "case13659pegase",
    "case_ACTIVSg10k", "case_ACTIVSg25k", "case_ACTIVSg70k", "case_SyntheticUSA",
    "case99k", "case193k",
]
PANDA_CASES = ["case2869pegase", "case9241pegase", "case13659pegase", "case193k"]
POWERWORLD_CASES = [
    "ACTIVSg200",
    "ACTIVSg2000 June 2016",
    "RTS-GMLC",
    "Texas7k (local TAMU copy)",
]
MATRIX_ROWS = [
    ("Bp sparse", "case118"),
    ("Bpp sparse", "case118"),
    ("Y_bus sparse", "case118"),
    ("LACPF block", "case118"),
    ("adjacency", "case118"),
    ("Bp sparse", "case2869pegase"),
    ("Bpp sparse", "case2869pegase"),
    ("Y_bus sparse", "case2869pegase"),
    ("LACPF block", "case2869pegase"),
    ("adjacency", "case2869pegase"),
    ("DC OPF incidence", "case118"),
    ("DC OPF weighted Laplacian", "case118"),
    ("DC OPF grounded Laplacian", "case118"),
    ("DC OPF flow map", "case118"),
    ("DC OPF instance", "case118"),
    ("PTDF + LODF", "case118"),
    ("pipeline Y_bus pair", "case2869pegase"),
]


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
        f"{ms(r['powerio_jl_ms'])} | {ms(r['exapowerio_ms'])} | "
        f"{ms(r['powermodels_ms'])} | {ms(r['rust_c_abi_ms'])} | {ms(r['powerio_data_ms'])} |"
        for r in selected
    ]
    return "\n".join(lines), []


def julia_ybus_rows(rows, cases):
    selected, missing = _select(rows, cases)
    if selected is None:
        return None, missing
    lines = [
        f"| {r['case']} | {r['buses']} / {r['branches']} | "
        f"{ms(r['powerio_jl_ybus_ms'])} | {ms(r['exapowerio_ybus_ms'])} | "
        f"{ms(r['rust_c_abi_ybus_arrow_ms'])} | {ms(r['powermodels_ybus_ms'])} |"
        for r in selected
    ]
    return "\n".join(lines), []


def panda_rows(rows, cases):
    selected, missing = _select(rows, cases)
    if selected is None:
        return None, missing
    lines = [
        f"| {r['case']} | {ms(r['powerio_parse_ms'])} | "
        f"{ms(r['powerio_matrix_ms'])} | {ms(r['matpowercaseframes_ms'])} |"
        for r in selected
    ]
    return "\n".join(lines), []


def powerworld_rows(rows, cases):
    selected, missing = _select(rows, cases)
    if selected is None:
        return None, missing
    lines = [
        f"| {r['case']} | {r['buses']} / {r['branches']} | "
        f"{ms(r['aux_ms'])} | {ms(r['pwb_ms'])} |"
        for r in selected
    ]
    return "\n".join(lines), []


def matrix_rows(rows, expected):
    by_key = {(r["operation"], r["case"]): r for r in rows}
    missing = [f"{op} / {case}" for op, case in expected if (op, case) not in by_key]
    if missing:
        return None, missing
    lines = []
    for op, case in expected:
        r = by_key[(op, case)]
        lines.append(
            f"| {r['operation']} | {r['case']} | {r['buses']} / {r['branches']} | {ms(r['ms'])} |"
        )
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
    speed_powerworld = load("speed_powerworld.json")
    speed_matrix = load("speed_matrix.json")

    # (region id, target file, table body or None, list of missing cases)
    plan = []
    if speed_julia is not None:
        body, missing = julia_rows(speed_julia["rows"], SPEED_JULIA_CASES)
        plan.append(("speed-julia", "benchmarks/RESULTS.md", SPEED_HEADER, body, missing))
        if "matrix_rows" in speed_julia:
            body, missing = julia_ybus_rows(speed_julia["matrix_rows"], SPEED_JULIA_CASES)
            plan.append(("speed-julia-ybus", "benchmarks/RESULTS.md", SPEED_YBUS_HEADER, body, missing))
    if speed_python is not None:
        body, missing = panda_rows(speed_python["rows"], PANDA_CASES)
        plan.append(("speed-pandapower", "benchmarks/RESULTS.md", PANDA_HEADER, body, missing))
    if speed_powerworld is not None:
        body, missing = powerworld_rows(speed_powerworld["rows"], POWERWORLD_CASES)
        plan.append(("powerworld", "benchmarks/RESULTS.md", POWERWORLD_HEADER, body, missing))
    if speed_matrix is not None:
        body, missing = matrix_rows(speed_matrix["rows"], MATRIX_ROWS)
        plan.append(("matrix", "benchmarks/RESULTS.md", MATRIX_HEADER, body, missing))

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

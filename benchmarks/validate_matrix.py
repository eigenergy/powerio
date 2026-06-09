#!/usr/bin/env python
"""Full reader x writer fidelity matrix, proven against independent oracles.

For each source case (a real native file wherever one exists), powerio converts it
to every target format, and the electrical core of each output is checked against
the core of the source itself, read by an independent oracle:

  - the source core comes from PowerModels.jl (MATPOWER, PSS/E) or the egret
    package (egret);
  - each output core comes from PowerModels.jl (MATPOWER / PowerModels JSON /
    PSS/E, and PowerWorld via a powerio .aux -> PowerModels JSON bridge, since no
    third-party .aux reader exists) or egret;
  - the diagonal (same-format) is checked byte-exact: the write echoes the source.

The core (bus/branch/gen counts and the per-unit demand/generation/shunt totals)
is preserved by every writer regardless of fidelity tier, so it is the right
invariant across the whole matrix. Generator cost, HVDC, and angle limits are
fidelity-tier specific and are covered by the dedicated checks
(validate_powermodels.jl field-by-field, validate_egret.py, and the Rust suite).

Source suites use representative cases per pair: basic, shunts, transformers,
size, an HVDC + mixed-gencost case, and a piecewise-cost case for MATPOWER; the
vendored real PSS/E `.raw` and egret `.json` files for those readers.

  egret must be importable (pip install gridx-egret). Run from anywhere:

  .venv/bin/python benchmarks/validate_matrix.py

Exits nonzero if any cell fails.
"""

import math
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import powerio  # noqa: E402

from validate_egret import core_pu as egret_core  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)

FORMATS = ["matpower", "powermodels-json", "psse", "powerworld", "egret-json"]
LABEL = {
    "matpower": "MAT",
    "powermodels-json": "PM",
    "psse": "PSS/E",
    "powerworld": "PWLD",
    "egret-json": "egret",
}
EXT = {
    "matpower": "m",
    "powermodels-json": "json",
    "psse": "raw",
    "powerworld": "aux",
    "egret-json": "egret.json",
}
CORE_FIELDS = [
    "n_bus", "n_branch", "n_gen", "n_load", "n_shunt",
    "sum_pd", "sum_qd", "sum_pg", "sum_gs", "sum_bs",
]
# Counts that must match exactly. load/shunt counts are informational: an oracle
# may aggregate per bus slightly differently, but a dropped element still shows up
# in the demand/shunt totals, which are strict.
STRICT_COUNTS = {"n_bus", "n_branch", "n_gen"}

# (source format, [files]). Real native files where they exist; representative
# MATPOWER cases otherwise (basic, shunts/transformers, size, HVDC + mixed
# gencost, piecewise cost, large).
SUITES = [
    ("matpower", [
        "tests/data/case9.m",
        "tests/data/case14.m",
        "tests/data/case30.m",
        "tests/data/case118.m",
        "tests/data/t_case9_dcline.m",
        "tests/data/pglib/pglib_opf_case5_pjm.m",
        "tests/data/case2869pegase.m",
    ]),
    ("psse", [
        "tests/data/psse/case5.raw",
        "tests/data/psse/case14.raw",
    ]),
    ("egret-json", [
        "tests/data/egret/case9.json",
        "tests/data/egret/case14.json",
        "tests/data/egret/case30.json",
        "tests/data/egret/dcline3.json",
    ]),
]


def convert(inp, to, frm):
    return powerio.convert_file(inp, to, frm).text


def write(text, path):
    with open(path, "w") as f:
        f.write(text)


def read(path):
    with open(path) as f:
        return f.read()


def pm_cores(paths):
    """Per-unit core of each PowerModels-readable file, in one Julia process."""
    if not paths:
        return {}
    out = subprocess.run(
        ["julia", "--project=benchmarks", "benchmarks/core_json.jl", *paths],
        cwd=ROOT, capture_output=True, text=True,
    )
    cores = {}
    for line in out.stdout.splitlines():
        path, _, rest = line.partition("\t")
        if rest.startswith("ERR") or not rest:
            cores[path] = None
            continue
        vals = rest.split()
        core = {}
        for i, field in enumerate(CORE_FIELDS):
            core[field] = int(vals[i]) if field.startswith("n_") else float(vals[i])
        cores[path] = core
    for p in paths:
        cores.setdefault(p, None)
    return cores


def diff_core(src, out):
    if src is None:
        return "source core unavailable"
    if out is None:
        return "output core unavailable (oracle could not read it)"
    problems = []
    for f in CORE_FIELDS:
        a, b = src[f], out[f]
        if f.startswith("n_"):
            if a != b and f in STRICT_COUNTS:
                problems.append(f"{f} {a}!={b}")
        elif not math.isclose(a, b, rel_tol=1e-6, abs_tol=1e-6):
            problems.append(f"{f} {a}!={b}")
    return "; ".join(problems)


def run_source(src_path, src_fmt, tmp):
    tag = os.path.basename(src_path).replace(".", "_")
    cells = {}

    outputs = {}  # T -> output path
    pm_needed = {}  # path -> T (PowerModels-readable, batched)
    egret_paths = {}  # T -> egret output path
    for t in FORMATS:
        out = os.path.join(tmp, f"{tag}__{t}.{EXT[t]}")
        write(convert(src_path, t, src_fmt), out)
        outputs[t] = out
        if t == src_fmt:
            cells[t] = (read(out) == read(src_path), "not a byte-exact echo")
        elif t in ("matpower", "powermodels-json", "psse"):
            pm_needed[out] = t
        elif t == "powerworld":
            bridge = os.path.join(tmp, f"{tag}__pwld.bridge.json")
            write(convert(out, "powermodels-json", "powerworld"), bridge)
            pm_needed[bridge] = t
        elif t == "egret-json":
            egret_paths[t] = out

    # Source core (its own oracle) plus all PowerModels-readable output cores.
    if src_fmt == "egret-json":
        src_core = egret_core(src_path)
        cores = pm_cores(list(pm_needed))
    else:
        batch = pm_cores([src_path, *pm_needed])
        src_core = batch.get(src_path)
        cores = batch

    for path, t in pm_needed.items():
        problems = diff_core(src_core, cores.get(path))
        cells[t] = (not problems, problems)
    for t, path in egret_paths.items():
        problems = diff_core(src_core, egret_core(path))
        cells[t] = (not problems, problems)

    return cells


def print_row(src_path, src_fmt, cells):
    name = os.path.basename(src_path)
    marks = "  ".join(
        f"{LABEL[t]}:{'ok' if cells[t][0] else 'FAIL'}" for t in FORMATS
    )
    print(f"  {name:<28} [{LABEL[src_fmt]}->]  {marks}")
    for t in FORMATS:
        ok, detail = cells[t]
        if not ok:
            print(f"      -> {LABEL[t]}: {detail}")


def main():
    import tempfile

    fails = 0
    cells_total = 0
    with tempfile.TemporaryDirectory() as tmp:
        for src_fmt, files in SUITES:
            print(f"\n=== source: {LABEL[src_fmt]} ===")
            for src_path in files:
                cells = run_source(src_path, src_fmt, tmp)
                print_row(src_path, src_fmt, cells)
                cells_total += len(cells)
                fails += sum(1 for ok, _ in cells.values() if not ok)

    print()
    if fails:
        print(f"{fails}/{cells_total} matrix cell(s) FAILED")
        sys.exit(1)
    print(f"all {cells_total} matrix cells passed")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Validate powerio's PyPSA CSV folder writer against PyPSA imports.

For each MATPOWER case, powerio writes a PyPSA CSV folder, PyPSA imports it, and
the script checks core counts, total load/generation, line r/x/b values
converted back to powerio's per unit basis, and transformer r/x/tap_ratio/s_nom
converted back from the transformer s_nom base. A line/transformer split
mismatch fails instead of skipping the value compare. Appends
`<stem>\tpypsa\t<mark>` to `$PIO_RESULTS_TSV` when set.
"""

import os
import shutil
import sys
import tempfile
import warnings
from pathlib import Path

import numpy as np

warnings.filterwarnings("ignore")

import powerio
import pypsa

ATOL = 1e-6
RTOL = 1e-6


def check_case(path: Path) -> str:
    case = powerio.parse_file(path)
    tmp = Path(tempfile.mkdtemp(prefix=f"powerio-pypsa-{path.stem}-"))
    problems = []
    try:
        case.write_pypsa_csv_folder(tmp)
        net = pypsa.Network()
        net.import_from_csv_folder(tmp)

        if len(net.buses) != case.n_buses:
            problems.append(f"bus count {len(net.buses)} != {case.n_buses}")
        if len(net.lines) + len(net.transformers) != case.n_branches:
            problems.append(f"branch count {len(net.lines) + len(net.transformers)} != {case.n_branches}")
        if len(net.generators) != case.n_gens:
            problems.append(f"generator count {len(net.generators)} != {case.n_gens}")
        if len(net.loads) != case.n_loads:
            problems.append(f"load count {len(net.loads)} != {case.n_loads}")

        p_load = float(net.loads.p_set.sum()) if len(net.loads) else 0.0
        q_load = float(net.loads.q_set.sum()) if len(net.loads) and "q_set" in net.loads else 0.0
        p_gen = float(net.generators.p_set.sum()) if len(net.generators) else 0.0
        want_p_load = sum(l["p"] for l in case.loads)
        want_q_load = sum(l["q"] for l in case.loads)
        want_p_gen = sum(g["pg"] for g in case.generators)
        check_close(problems, "total load p", p_load, want_p_load)
        check_close(problems, "total load q", q_load, want_q_load)
        check_close(problems, "total gen p", p_gen, want_p_gen)

        line_branches = [
            b for b in case.branches if b["tap"] == 0.0 and b["shift"] == 0.0
        ]
        if len(net.lines) == len(line_branches):
            py_r = []
            py_x = []
            py_b = []
            for _, row in net.lines.iterrows():
                v = float(net.buses.loc[row.bus1].v_nom)
                zb = zbase(v, case.base_mva)
                py_r.append(float(row.r) / zb)
                py_x.append(float(row.x) / zb)
                py_b.append(float(row.b) * zb)
            check_vec(problems, "line.r", py_r, [b["r"] for b in line_branches])
            check_vec(problems, "line.x", py_x, [b["x"] for b in line_branches])
            check_vec(problems, "line.b", py_b, [b["b"] for b in line_branches])
        else:
            problems.append(
                f"line/transformer split: {len(net.lines)} lines != {len(line_branches)} expected"
            )

        xf_branches = [
            b for b in case.branches if b["tap"] != 0.0 or b["shift"] != 0.0
        ]
        if len(net.transformers) == len(xf_branches):
            xf_r = []
            xf_x = []
            xf_tap = []
            for name, row in net.transformers.iterrows():
                s_nom = float(row.s_nom)
                if s_nom <= 0.0:
                    problems.append(f"transformer {name}: s_nom {s_nom} <= 0")
                    s_nom = case.base_mva  # keep the vector compare aligned
                # Written per unit on the transformer s_nom base; convert back.
                xf_r.append(float(row.r) * case.base_mva / s_nom)
                xf_x.append(float(row.x) * case.base_mva / s_nom)
                xf_tap.append(float(row.tap_ratio))
            check_vec(problems, "transformer.r", xf_r, [b["r"] for b in xf_branches])
            check_vec(problems, "transformer.x", xf_x, [b["x"] for b in xf_branches])
            check_vec(
                problems,
                "transformer.tap_ratio",
                xf_tap,
                [b["tap"] if b["tap"] != 0.0 else 1.0 for b in xf_branches],
            )
            # rate_a == 0 (unlimited) writes s_nom = base_mva, so only compare
            # s_nom where the branch carries a limit.
            for (_, row), b in zip(net.transformers.iterrows(), xf_branches):
                if b["rate_a"] > 0.0:
                    check_close(
                        problems, "transformer.s_nom", float(row.s_nom), b["rate_a"]
                    )
        else:
            problems.append(
                f"line/transformer split: {len(net.transformers)} transformers"
                f" != {len(xf_branches)} expected"
            )
    except Exception as exc:  # noqa: BLE001
        problems.append(f"PyPSA import/check failed: {type(exc).__name__}: {exc}")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    if problems:
        print(f"MISMATCH: {path.name} PyPSA CSV")
        for p in problems[:30]:
            print("  ", p)
        return "FAIL"
    print(f"MATCH: {path.name} PyPSA CSV counts and branch parameters")
    return "ok"


def zbase(v_kv, base_mva):
    # Mirror the writer's guard: a bus with no base_kv (the IEEE cases ship
    # base_kv 0) uses zbase = 1, i.e. ohms == per unit.
    if v_kv > 0.0 and base_mva > 0.0:
        return v_kv * v_kv / base_mva
    return 1.0


def check_close(problems, label, got, want):
    if not np.isclose(got, want, atol=ATOL, rtol=RTOL):
        problems.append(f"{label}: {got} != {want}")


def check_vec(problems, label, got, want):
    got = np.asarray(got, dtype=float)
    want = np.asarray(want, dtype=float)
    if got.shape != want.shape:
        problems.append(f"{label}: shape {got.shape} != {want.shape}")
        return
    bad = ~np.isclose(got, want, atol=ATOL, rtol=RTOL, equal_nan=True)
    if bad.any():
        i = int(np.argmax(bad))
        problems.append(f"{label}: {int(bad.sum())} differ, first {got[i]} != {want[i]}")


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: validate_pypsa.py <case.m> [case.m ...]", file=sys.stderr)
        return 2
    results = os.environ.get("PIO_RESULTS_TSV")
    fails = 0
    for arg in sys.argv[1:]:
        mark = check_case(Path(arg))
        if mark == "FAIL":
            fails += 1
        if results:
            with open(results, "a") as fh:
                fh.write(f"{Path(arg).stem}\tpypsa\t{mark}\n")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())


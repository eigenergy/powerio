#!/usr/bin/env python3
"""Benchmark casemat against matpowercaseframes on a large MATPOWER case.

casemat parses *and* builds matrices (Y_bus, B'); matpowercaseframes only parses
the case into pandas DataFrames. So the "parse" rows are the apples-to-apples
comparison, and "parse + Y_bus + B'" is casemat's full path against issue #5's
100 ms target for case2869pegase.

    python benchmarks/bench_parse.py [path/to/case.m]

Install the comparison baseline with `pip install 'casemat[bench]'`.
"""

import statistics
import sys
import time
from pathlib import Path

import casemat as nm

DEFAULT_CASE = (
    Path(__file__).resolve().parent.parent / "tests" / "data" / "case2869pegase.m"
)


def best_median(fn, n=25, warmup=5):
    for _ in range(warmup):
        fn()
    samples = []
    for _ in range(n):
        start = time.perf_counter()
        fn()
        samples.append(time.perf_counter() - start)
    return min(samples) * 1e3, statistics.median(samples) * 1e3


def main():
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_CASE
    case = nm.parse_matpower(str(path))
    print(
        f"case {path.name}: {case.n} buses, {case.n_branches} branches, "
        f"{case.n_gens} gens\n"
    )

    rows = [
        ("casemat: parse", *best_median(lambda: nm.parse_matpower(str(path)))),
    ]

    def full_path():
        c = nm.parse_matpower(str(path))
        c.ybus()
        c.bprime()

    rows.append(("casemat: parse + Y_bus + B'", *best_median(full_path)))

    try:
        from matpowercaseframes import CaseFrames

        rows.append(
            ("matpowercaseframes: parse", *best_median(lambda: CaseFrames(str(path))))
        )
    except ImportError as exc:
        # A present-but-broken install should show its own error, not "skipping".
        if getattr(exc, "name", None) not in ("matpowercaseframes", None):
            raise
        print("matpowercaseframes not installed; skipping the baseline row.")
        print("  pip install 'casemat[bench]'\n")

    # pandapower reads .m via matpowercaseframes, then builds its `net`. This is
    # the apples-to-apples "convert a MATPOWER file into the tool's model" row.
    try:
        from pandapower.converter import from_mpc

        rows.append(("pandapower: from_mpc", *best_median(lambda: from_mpc(str(path)))))
    except ImportError:
        print("pandapower not installed; skipping the pandapower row.")
        print("  pip install pandapower matpowercaseframes\n")
    except Exception as exc:  # noqa: BLE001 - from_mpc raises on some cases (pp 3.2.2)
        print(f"pandapower from_mpc failed on this case: {type(exc).__name__}\n")

    width = max(len(name) for name, *_ in rows)
    print(f"{'task':<{width}}  {'best (ms)':>10}  {'median (ms)':>12}")
    print("-" * (width + 26))
    for name, best, median in rows:
        print(f"{name:<{width}}  {best:>10.1f}  {median:>12.1f}")


if __name__ == "__main__":
    main()

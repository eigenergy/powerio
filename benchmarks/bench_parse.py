#!/usr/bin/env python3
"""Benchmark the powerio Python package against pandapower's stack.

Four rows, from leanest to fullest:

- ``powerio: parse`` — the zero-dependency parser (no numpy/scipy). This is the
  apples-to-apples number against matpowercaseframes: parse a MATPOWER file into
  the tool's in-memory model, nothing more.
- ``powerio[matrix]: parse + Y_bus + B'`` — powerio's parse plus building the two
  matrices scipy callers usually want, against issue #5's 100 ms target for
  case2869pegase.
- ``matpowercaseframes: parse`` — pandapower's ``.m`` reader (pandas DataFrames).
- ``pandapower: from_mpc`` — the full convert-into-``net`` path.

    python benchmarks/bench_parse.py [path/to/case.m ...]

Run it with the venv that has the extensions built (`maturin develop --release`).
Install the comparison baselines with `pip install 'powerio[bench]'`.
"""

import json
import logging
import statistics
import sys
import time
import warnings
from pathlib import Path

import powerio

# pandapower/matpowercaseframes emit a wall of dtype FutureWarnings, mixed-cost
# UserWarnings, and logger lines that drown the table; none are ours to fix.
warnings.filterwarnings("ignore", category=FutureWarning)
warnings.filterwarnings("ignore", category=UserWarning)
logging.getLogger("pandapower").setLevel(logging.ERROR)

# from_mpc builds a full `net`; above this many buses it's minutes per call and
# errors on some topologies, so skip it and keep the matpowercaseframes baseline.
FROM_MPC_MAX_BUSES = 25_000

DEFAULT_CASES = [
    Path(__file__).resolve().parent.parent / "tests" / "data" / "case2869pegase.m"
]


def best_median(fn, n, warmup):
    for _ in range(warmup):
        fn()
    samples = []
    for _ in range(n):
        start = time.perf_counter()
        fn()
        samples.append(time.perf_counter() - start)
    return min(samples) * 1e3, statistics.median(samples) * 1e3


def samples_for(nbuses):
    """Fewer reps on the big cases, where one parse already takes ~seconds."""
    if nbuses > 50_000:
        return 3, 1
    if nbuses > 10_000:
        return 5, 2
    return 25, 5


def bench_case(path: Path):
    case = powerio.parse(str(path))
    print(
        f"case {path.name}: {case.n} buses, {case.n_branches} branches, "
        f"{case.n_gens} gens"
    )
    n, warm = samples_for(case.n)

    def timed(fn):
        return best_median(fn, n, warm)

    rows = [("powerio: parse", *timed(lambda: powerio.parse(str(path))))]

    def full_path():
        c = powerio.parse_matpower(str(path))
        c.ybus()
        c.bprime()

    rows.append(("powerio[matrix]: parse + Y_bus + B'", *timed(full_path)))

    try:
        from matpowercaseframes import CaseFrames

        rows.append(("matpowercaseframes: parse", *timed(lambda: CaseFrames(str(path)))))
    except ImportError as exc:
        # A present-but-broken install should show its own error, not "skipping".
        if getattr(exc, "name", None) not in ("matpowercaseframes", None):
            raise
        print("matpowercaseframes not installed; skipping the baseline row.")
        print("  pip install 'powerio[bench]'")

    # pandapower reads .m via matpowercaseframes, then builds its `net`. This is
    # the apples-to-apples "convert a MATPOWER file into the tool's model" row.
    if case.n > FROM_MPC_MAX_BUSES:
        print(f"pandapower from_mpc skipped above {FROM_MPC_MAX_BUSES} buses.")
    else:
        try:
            from pandapower.converter import from_mpc

            rows.append(("pandapower: from_mpc", *timed(lambda: from_mpc(str(path)))))
        except ImportError:
            print("pandapower not installed; skipping the pandapower row.")
            print("  pip install pandapower matpowercaseframes")
        except Exception as exc:  # noqa: BLE001 - from_mpc raises on some cases (pp 3.2.2)
            print(f"pandapower from_mpc failed on this case: {type(exc).__name__}")

    width = max(len(name) for name, *_ in rows)
    print(f"{'task':<{width}}  {'best (ms)':>10}  {'median (ms)':>12}")
    print("-" * (width + 26))
    for name, best, median in rows:
        print(f"{name:<{width}}  {best:>10.1f}  {median:>12.1f}")
    print()

    # The two rows render_tables.py needs for the RESULTS pandapower table; round to
    # the 1 decimal the published table shows. matpowercaseframes is None when its
    # baseline isn't installed.
    medians = {name: median for name, _, median in rows}
    return {
        "case": path.stem,
        "powerio_parse_ms": round(medians["powerio: parse"], 1),
        "matpowercaseframes_ms": round(medians["matpowercaseframes: parse"], 1)
        if "matpowercaseframes: parse" in medians else None,
    }


def main():
    args = sys.argv[1:]
    json_out = "--json" in args
    paths = [Path(a) for a in args if a != "--json"] or DEFAULT_CASES
    results = [bench_case(path) for path in paths]
    if json_out:
        out = Path(__file__).resolve().parent / "results" / "speed_python.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps({"rows": results}, indent=2) + "\n")
        print(f"wrote {out} ({len(results)} rows)")


if __name__ == "__main__":
    main()

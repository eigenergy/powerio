#!/usr/bin/env python3
"""Benchmark the powerio Python package against pandapower's stack.

Four rows, from leanest to fullest:

- ``powerio: parse`` — the zero-dependency parser (no numpy/scipy). This is the
  apples-to-apples number against matpowercaseframes: parse a MATPOWER file into
  the tool's in-memory model, nothing more.
- ``powerio[matrix]: parse + Y_bus + Bp`` — powerio's parse plus building the two
  matrices scipy callers usually want, against issue #5's 100 ms target for
  case2869pegase.
- ``matpowercaseframes: parse`` — pandapower's ``.m`` reader (pandas DataFrames).
- ``pandapower: from_mpc`` — the full convert-into-``net`` path.

    python benchmarks/bench_parse.py [path/to/case.m ...]

Run it with the venv that has the extension and benchmark baselines installed:

    .venv/bin/python -m pip install --upgrade pip maturin -r benchmarks/requirements.txt
    env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
"""

import json
import logging
import subprocess
import statistics
import sys
import time
import warnings
from datetime import datetime, timezone
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


def sample_stats(fn, n, warmup):
    for _ in range(warmup):
        fn()
    samples = []
    for _ in range(n):
        start = time.perf_counter()
        fn()
        samples.append((time.perf_counter() - start) * 1e3)
    return {
        "best": min(samples),
        "median": statistics.median(samples),
        "std": statistics.stdev(samples) if len(samples) > 1 else 0.0,
        "n": len(samples),
    }


def benchmark_metadata(args):
    repo = Path(__file__).resolve().parent.parent
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=repo,
            check=False,
            capture_output=True,
            text=True,
        ).stdout.strip() or None
    except OSError:
        commit = None
    return {
        "benchmark_time_utc": datetime.now(timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z"),
        "git_commit": commit,
        "command": " ".join(["python", "benchmarks/bench_parse.py"] + args),
    }


def samples_for(nbuses):
    """Fewer reps on the big cases, where one parse already takes ~seconds."""
    if nbuses > 50_000:
        return 3, 1
    if nbuses > 10_000:
        return 5, 2
    return 25, 5


def bench_case(path: Path):
    case = powerio.parse_file(str(path))
    print(
        f"case {path.name}: {case.n_buses} buses, {case.n_branches} branches, "
        f"{case.n_gens} gens"
    )
    n, warm = samples_for(case.n_buses)

    def timed(fn):
        return sample_stats(fn, n, warm)

    rows = [("powerio: parse", timed(lambda: powerio.parse_file(str(path))))]

    def full_path():
        c = powerio.parse_file(str(path))
        c.ybus()
        c.bprime()

    rows.append(("powerio[matrix]: parse + Y_bus + Bp", timed(full_path)))

    try:
        from matpowercaseframes import CaseFrames

        rows.append(("matpowercaseframes: parse", timed(lambda: CaseFrames(str(path)))))
    except ImportError as exc:
        if getattr(exc, "name", None) not in ("matpowercaseframes", None):
            raise
        raise RuntimeError(
            "matpowercaseframes is required for the published Python benchmark; "
            "install benchmarks/requirements.txt into the same venv"
        ) from exc
    except Exception as exc:  # noqa: BLE001 - baseline readers raise on some cases
        print(
            "matpowercaseframes failed on this case: "
            f"{type(exc).__name__}: {exc}"
        )

    # pandapower reads .m via matpowercaseframes, then builds its `net`. This is
    # the apples-to-apples "convert a MATPOWER file into the tool's model" row.
    if case.n_buses > FROM_MPC_MAX_BUSES:
        print(f"pandapower from_mpc skipped above {FROM_MPC_MAX_BUSES} buses.")
    else:
        try:
            from pandapower.converter import from_mpc

            rows.append(("pandapower: from_mpc", timed(lambda: from_mpc(str(path)))))
        except ImportError as exc:
            raise RuntimeError(
                "pandapower is required for the published Python benchmark; "
                "install benchmarks/requirements.txt into the same venv"
            ) from exc
        except Exception as exc:  # noqa: BLE001 - from_mpc raises on some cases (pp 3.2.2)
            print(f"pandapower from_mpc failed on this case: {type(exc).__name__}: {exc}")

    width = max(len(name) for name, _ in rows)
    print(f"{'task':<{width}}  {'best (ms)':>10}  {'median (ms)':>12}  {'std (ms)':>10}  {'n':>4}")
    print("-" * (width + 44))
    for name, stats in rows:
        print(
            f"{name:<{width}}  {stats['best']:>10.1f}  {stats['median']:>12.1f}  "
            f"{stats['std']:>10.1f}  {stats['n']:>4}"
        )
    print()

    # Rows render_tables.py needs for the RESULTS pandapower table; round to the
    # 1 decimal the published table shows. matpowercaseframes is None when its
    # baseline is unavailable for this case.
    stats_by_name = {name: stats for name, stats in rows}

    def rounded(name, field):
        if name not in stats_by_name:
            return None
        return round(stats_by_name[name][field], 1)

    def count(name):
        if name not in stats_by_name:
            return 0
        return stats_by_name[name]["n"]

    return {
        "case": path.stem,
        "powerio_parse_ms": rounded("powerio: parse", "median"),
        "powerio_parse_std_ms": rounded("powerio: parse", "std"),
        "powerio_parse_n": count("powerio: parse"),
        "powerio_matrix_ms": rounded("powerio[matrix]: parse + Y_bus + Bp", "median"),
        "powerio_matrix_std_ms": rounded("powerio[matrix]: parse + Y_bus + Bp", "std"),
        "powerio_matrix_n": count("powerio[matrix]: parse + Y_bus + Bp"),
        "matpowercaseframes_ms": rounded("matpowercaseframes: parse", "median"),
        "matpowercaseframes_std_ms": rounded("matpowercaseframes: parse", "std"),
        "matpowercaseframes_n": count("matpowercaseframes: parse"),
    }


def main():
    args = sys.argv[1:]
    json_out = "--json" in args
    paths = [Path(a) for a in args if a != "--json"] or DEFAULT_CASES
    results = [bench_case(path) for path in paths]
    if json_out:
        out = Path(__file__).resolve().parent / "results" / "speed_python.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(
            json.dumps(
                {"metadata": benchmark_metadata(args), "rows": results},
                indent=2,
            )
            + "\n"
        )
        print(f"wrote {out} ({len(results)} rows)")


if __name__ == "__main__":
    main()

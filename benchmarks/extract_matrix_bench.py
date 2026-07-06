#!/usr/bin/env python3
"""Write benchmarks/results/speed_matrix.json from Criterion estimates.

Run after:

    cargo bench -p powerio-matrix --bench matrix

The benchmark parses and indexes fixtures outside the timed loop. These rows
therefore measure derived matrix construction, not file parsing. The output
feeds benchmarks/render_tables.py. Stdlib only; does not import powerio.
"""

import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CRITERION = REPO / "target" / "criterion"
OUT = REPO / "benchmarks" / "results" / "speed_matrix.json"


def row(operation, case, buses, branches, bench):
    return {
        "operation": operation,
        "case": case,
        "buses": buses,
        "branches": branches,
        "bench": bench,
    }


ROWS = [
    row("Bp sparse", "case118", 118, 186, "matrix_bprime_case118"),
    row("Bpp sparse", "case118", 118, 186, "matrix_bdoubleprime_case118"),
    row("Y_bus sparse", "case118", 118, 186, "matrix_ybus_case118"),
    row("LACPF block", "case118", 118, 186, "matrix_lacpf_case118"),
    row("adjacency", "case118", 118, 186, "matrix_adjacency_case118"),
    row("Bp sparse", "case2869pegase", 2869, 4582, "matrix_bprime_case2869pegase"),
    row(
        "Bpp sparse",
        "case2869pegase",
        2869,
        4582,
        "matrix_bdoubleprime_case2869pegase",
    ),
    row("Y_bus sparse", "case2869pegase", 2869, 4582, "matrix_ybus_case2869pegase"),
    row("LACPF block", "case2869pegase", 2869, 4582, "matrix_lacpf_case2869pegase"),
    row("adjacency", "case2869pegase", 2869, 4582, "matrix_adjacency_case2869pegase"),
    row("DC OPF incidence", "case118", 118, 186, "dcopf_incidence_case118"),
    row(
        "DC OPF weighted Laplacian",
        "case118",
        118,
        186,
        "dcopf_laplacian_case118",
    ),
    row(
        "DC OPF grounded Laplacian",
        "case118",
        118,
        186,
        "dcopf_grounded_laplacian_case118",
    ),
    row("DC OPF flow map", "case118", 118, 186, "dcopf_flow_map_case118"),
    row("DC OPF instance", "case118", 118, 186, "dcopf_instance_case118"),
    row("PTDF + LODF", "case118", 118, 186, "sensitivity_ptdf_lodf_case118"),
    row(
        "pipeline Y_bus pair",
        "case2869pegase",
        2869,
        4582,
        "pipeline_ybus_pair_case2869pegase",
    ),
]


def estimate(bench):
    path = CRITERION / bench / "new" / "estimates.json"
    if not path.exists():
        raise SystemExit(f"missing Criterion estimate: {path}")
    data = json.loads(path.read_text())
    center = data.get("median", data["mean"])["point_estimate"]
    std = data.get("std_dev", {}).get("point_estimate")
    samples = CRITERION / bench / "new" / "sample.json"
    n = 0
    if samples.exists():
        sample_data = json.loads(samples.read_text())
        n = len(sample_data.get("times", []))
    return {
        "ms": round(center / 1_000_000, 4),
        "std_ms": round(std / 1_000_000, 5) if std is not None else None,
        "n": n,
    }


def metadata():
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPO,
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
        "command": "cargo bench -p powerio-matrix --bench matrix && python3 benchmarks/extract_matrix_bench.py",
    }


def main():
    rows = []
    for row in ROWS:
        stats = estimate(row["bench"])
        rows.append(
            {
                "operation": row["operation"],
                "case": row["case"],
                "buses": row["buses"],
                "branches": row["branches"],
                "ms": stats["ms"],
                "std_ms": stats["std_ms"],
                "n": stats["n"],
            }
        )
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"metadata": metadata(), "rows": rows}, indent=2) + "\n")
    print(f"wrote {OUT} ({len(rows)} rows)")


if __name__ == "__main__":
    main()

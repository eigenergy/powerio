#!/usr/bin/env python3
"""Write benchmarks/results/speed_powerworld.json from Criterion estimates.

Run after:

    POWERIO_BENCH_AUX=/path/to/Texas7k_20210804.AUX \
      POWERIO_BENCH_PWB=/path/to/Texas7k_20210804.PWB \
      cargo bench -p powerio --bench parse -- "parse_aux_|parse_pwb_"

The output feeds benchmarks/render_tables.py. Stdlib only; does not import
powerio.
"""

import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CRITERION = REPO / "target" / "criterion"
OUT = REPO / "benchmarks" / "results" / "speed_powerworld.json"

ROWS = [
    {
        "case": "ACTIVSg200",
        "buses": 200,
        "branches": 246,
        "aux": "parse_aux_activsg200",
        "pwb": "parse_pwb_activsg200",
    },
    {
        "case": "ACTIVSg2000 June 2016",
        "buses": 2007,
        "branches": 3043,
        "aux": "parse_aux_activsg2000",
        "pwb": "parse_pwb_activsg2000",
    },
    {
        "case": "RTS-GMLC",
        "buses": 73,
        "branches": 120,
        "aux": None,
        "pwb": "parse_pwb_rts_gmlc",
    },
    {
        "case": "Texas7k (local TAMU copy)",
        "buses": 6717,
        "branches": 9140,
        "aux": "parse_aux_extra",
        "pwb": "parse_pwb_extra",
    },
]


def estimate(bench):
    if bench is None:
        return None, None, 0
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
    return (
        round(center / 1_000_000, 2),
        round(std / 1_000_000, 2) if std is not None else None,
        n,
    )


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
        "command": 'POWERIO_BENCH_AUX=<Texas7k_20210804.AUX> POWERIO_BENCH_PWB=<Texas7k_20210804.PWB> cargo bench -p powerio --bench parse -- "parse_aux_|parse_pwb_" && python3 benchmarks/extract_powerworld_bench.py',
    }


def main():
    rows = []
    for row in ROWS:
        aux_ms, aux_std_ms, aux_n = estimate(row["aux"])
        pwb_ms, pwb_std_ms, pwb_n = estimate(row["pwb"])
        rows.append(
            {
                "case": row["case"],
                "buses": row["buses"],
                "branches": row["branches"],
                "aux_ms": aux_ms,
                "aux_std_ms": aux_std_ms,
                "aux_n": aux_n,
                "pwb_ms": pwb_ms,
                "pwb_std_ms": pwb_std_ms,
                "pwb_n": pwb_n,
            }
        )
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"metadata": metadata(), "rows": rows}, indent=2) + "\n")
    print(f"wrote {OUT} ({len(rows)} rows)")


if __name__ == "__main__":
    main()

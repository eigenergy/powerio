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


def ms(bench):
    if bench is None:
        return None
    path = CRITERION / bench / "new" / "estimates.json"
    if not path.exists():
        raise SystemExit(f"missing Criterion estimate: {path}")
    data = json.loads(path.read_text())
    return round(data["mean"]["point_estimate"] / 1_000_000, 2)


def main():
    rows = [
        {
            "case": row["case"],
            "buses": row["buses"],
            "branches": row["branches"],
            "aux_ms": ms(row["aux"]),
            "pwb_ms": ms(row["pwb"]),
        }
        for row in ROWS
    ]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"rows": rows}, indent=2) + "\n")
    print(f"wrote {OUT} ({len(rows)} rows)")


if __name__ == "__main__":
    main()

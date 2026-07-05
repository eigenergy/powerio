#!/usr/bin/env python3
"""Write benchmarks/results/speed_matrix.json from Criterion estimates.

Run after:

    cargo bench -p powerio-matrix --bench matrix

The benchmark parses and indexes fixtures outside the timed loop. These rows
therefore measure derived matrix construction, not file parsing. The output
feeds benchmarks/render_tables.py. Stdlib only; does not import powerio.
"""

import json
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


def ms(bench):
    path = CRITERION / bench / "new" / "estimates.json"
    if not path.exists():
        raise SystemExit(f"missing Criterion estimate: {path}")
    data = json.loads(path.read_text())
    return round(data["mean"]["point_estimate"] / 1_000_000, 3)


def main():
    rows = [
        {
            "operation": row["operation"],
            "case": row["case"],
            "buses": row["buses"],
            "branches": row["branches"],
            "ms": ms(row["bench"]),
        }
        for row in ROWS
    ]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"rows": rows}, indent=2) + "\n")
    print(f"wrote {OUT} ({len(rows)} rows)")


if __name__ == "__main__":
    main()

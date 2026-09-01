#!/usr/bin/env python3
"""Validate PowerIO's GOC3 fixtures with the pinned GO-3 data model."""

from __future__ import annotations

import argparse
from pathlib import Path

from datamodel.input.data import InputDataFile
from datamodel.output.data import OutputDataFile


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--benchmark-data", type=Path, required=True)
    parser.add_argument("--powerio-problem", type=Path, required=True)
    parser.add_argument("--powerio-solution", type=Path, required=True)
    args = parser.parse_args()

    problems = sorted(args.benchmark_data.glob("*.json"))
    if len(problems) != 3:
        raise SystemExit(
            f"expected the pinned D1, D2, and D3 files; found {len(problems)}"
        )

    for path in [args.powerio_problem, *problems]:
        InputDataFile.load(str(path))
        print(f"validated GOC3 problem: {path.name}")

    OutputDataFile.load(str(args.powerio_solution))
    print(f"validated GOC3 solution: {args.powerio_solution.name}")


if __name__ == "__main__":
    main()

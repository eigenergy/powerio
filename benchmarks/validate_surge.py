#!/usr/bin/env python
"""Optional Surge oracle for powerio's Surge JSON writer.

Usage:
  SURGE_BIN=/path/to/surge-solve benchmarks/validate_surge.py ref.m out.surge.json
  SURGE_CHECKOUT=/path/to/surge benchmarks/validate_surge.py ref.m out.surge.json

The oracle loads powerio's `.surge.json` output with Surge's own CLI
`--parse-only --output json` and compares counts and totals against the
reference case parsed by powerio. It is benchmark scoped: no Surge crate or
Python package is a dependency of powerio.
"""

from __future__ import annotations

import json
import math
import os
import subprocess
import sys
from pathlib import Path

import powerio


def surge_command() -> list[str] | None:
    if bin_path := os.environ.get("SURGE_BIN"):
        return [bin_path]
    if checkout := os.environ.get("SURGE_CHECKOUT"):
        manifest = Path(checkout) / "Cargo.toml"
        return [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(manifest),
            "-p",
            "surge-bindings",
            "--bin",
            "surge-solve",
            "--",
        ]
    return None


def powerio_core(path: str) -> dict[str, float]:
    case = powerio.parse(path)
    return {
        "n_buses": case.n,
        "n_branches": case.n_branches,
        "n_generators": case.n_gens,
        "total_load_mw": sum(load["p"] for load in case.loads),
        "total_gen_mw": sum(gen["pg"] for gen in case.gens),
        "base_mva": case.base_mva,
    }


def surge_core(path: str) -> dict[str, float]:
    cmd = surge_command()
    if cmd is None:
        print("SKIP: set SURGE_BIN or SURGE_CHECKOUT for the Surge oracle")
        raise SystemExit(77)
    out = subprocess.run(
        [*cmd, path, "--parse-only", "--output", "json"],
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        print(out.stderr or out.stdout, file=sys.stderr)
        raise SystemExit(out.returncode)
    data = json.loads(out.stdout)
    return {
        "n_buses": data["n_buses"],
        "n_branches": data["n_branches"],
        "n_generators": data["n_generators"],
        "total_load_mw": data["total_load_mw"],
        "total_gen_mw": data["total_gen_mw"],
        "base_mva": data["base_mva"],
    }


def main() -> None:
    if len(sys.argv) != 3:
        print("usage: validate_surge.py <reference case> <powerio surge json>", file=sys.stderr)
        raise SystemExit(2)
    ref, out = sys.argv[1], sys.argv[2]
    ref_core = powerio_core(ref)
    out_core = surge_core(out)
    problems = []
    for key in ("n_buses", "n_branches", "n_generators"):
        if int(ref_core[key]) != int(out_core[key]):
            problems.append(f"{key} {ref_core[key]}!={out_core[key]}")
    for key in ("total_load_mw", "total_gen_mw", "base_mva"):
        if not math.isclose(ref_core[key], out_core[key], rel_tol=1e-6, abs_tol=1e-6):
            problems.append(f"{key} {ref_core[key]}!={out_core[key]}")
    if problems:
        print("; ".join(problems), file=sys.stderr)
        raise SystemExit(1)
    print("ok")


if __name__ == "__main__":
    main()

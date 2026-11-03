"""Measure native result sensitivity to declaration order without changing physics.

Consumes canonical outputs of ieee_distribution_import_oracle.py. This is a
numerical diagnostic, never feeder qualification or a tolerance adjustment.
Every native component inventory and primitive admittance must remain exact.
"""

import argparse
import hashlib
import json
import shutil
from collections import Counter
from pathlib import Path

from european_lv_import_oracle import snapshot
from ieee4_import_oracle import run
from oracle import compare

DEFAULT_CASES = ("ieee34_mod2", "ieee13", "ieee123", "ieee123_switches")


def primitive_inventory(engine):
    result = {}
    for name in engine.Circuit.AllElementNames():
        engine.Circuit.SetActiveElement(name)
        result[name.lower()] = dict(
            enabled=engine.CktElement.Enabled(),
            buses=[bus.lower() for bus in engine.CktElement.BusNames()],
            nodes=engine.CktElement.NodeOrder(),
            conductors=engine.CktElement.NumConductors(),
            terminals=engine.CktElement.NumTerminals(),
            yprim=engine.CktElement.YPrim(),
        )
    return result


def reordered(lines, variant):
    result = list(lines)
    if variant == "reverse_lines":
        positions = [i for i, line in enumerate(result) if line.lower().startswith("new line.")]
        values = [result[i] for i in reversed(positions)]
        for i, value in zip(positions, values, strict=True):
            result[i] = value
    elif variant == "transformers_first":
        moved = [line for line in result if line.lower().startswith("new transformer.")]
        result = [line for line in result if line not in moved]
        first_line = next(i for i, line in enumerate(result) if line.lower().startswith("new line."))
        result[first_line:first_line] = moved
    else:
        raise ValueError(f"unknown permutation {variant}")
    if Counter(result) != Counter(lines):
        raise ValueError("permutation changed declarations")
    return "\n".join(result) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-directory", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append", help="repeat to select cases")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    evidence = args.run_directory / "report.json"
    report = dict(scope=__doc__, feeder_report_sha256=hashlib.sha256(evidence.read_bytes()).hexdigest(), cases={})
    for case in args.case or DEFAULT_CASES:
        source = args.run_directory / case / "imported/dss"
        folder = args.output / case
        shutil.copytree(source, folder)
        path = folder / "case.dss"
        text = path.read_text()
        native = run(path, 1e-11)
        buses = [bus.lower() for bus in native.Circuit.AllBusNames()]
        elements = [name.lower() for name in native.Circuit.AllElementNames()]
        primitives = primitive_inventory(native)
        expected = snapshot(native, buses, elements, canonical=False)
        row = dict(engine=native.Basic.Version(), source_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                   solver_tolerance=1e-11, observables=len(expected), components=len(primitives), variants={})
        for variant in ("transformers_first", "reverse_lines"):
            target = folder / f"{variant}.dss"
            target.write_text(reordered(text.splitlines(), variant))
            engine = run(target, 1e-11)
            actual_primitives = primitive_inventory(engine)
            exact = actual_primitives == primitives
            differences = [name for name in sorted(primitives.keys() | actual_primitives.keys())
                           if primitives.get(name) != actual_primitives.get(name)]
            actual = snapshot(engine, buses, elements, canonical=False)
            comparison = compare(expected, actual)
            row["variants"][variant] = dict(
                source_sha256=hashlib.sha256(target.read_bytes()).hexdigest(),
                identical_declarations=True, exact_native_primitives=exact,
                changed_primitive_elements=differences, comparison=comparison,
                exceeds_strict_threshold_with_identical_primitives=exact and not comparison["passed"],
            )
            print(case, variant, "exact primitives", exact, "strict failures", len(comparison["failures"]), flush=True)
        report["cases"][case] = row
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()

"""Reproduce the pinned PowerModelsACDC MATPOWER-extension IO qualification."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
# PGLib-HVDC commit f928f95cde06d035554d92854f03950e074850e0.
CASES = [
    ("case5_3_he", "759606ec335f8706ee8631ff4833c50804dd916afb5b5e682ace4f4f886fd7f3", "pf,opf"),
    ("case24_7_jb", "fe8c31d586ef6853257a0ec9a018c2b92da030808e58e57b309deac1eb5ae4b4", "pf,opf"),
    ("case39_10_he", "678a3c76c36663aef7938117d77e1a9c66bd466ebcbe98405530e3a52b6f212d", "opf"),
    ("case67", "1f7637a093a5c9968f5571b323875a57f60f319a665600e0b5e11d9463c9cbf1", "opf"),
]


def run(command):
    subprocess.run([str(arg) for arg in command], cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--pglib-dir", type=Path, help="directory containing the three external pinned cases")
    args = parser.parse_args()
    output = args.output.resolve()
    corpus = args.pglib_dir.resolve() if args.pglib_dir else None
    selected = CASES if corpus else CASES[:1]
    # Check all inputs before creating outputs or running a solver.
    inputs = []
    for name, digest, modes in selected:
        source = ROOT / "tests/data/pglib-hvdc/case5_3_he.m" if name == "case5_3_he" else corpus / f"{name}.m"
        actual = hashlib.sha256(source.read_bytes()).hexdigest()
        if actual != digest:
            raise ValueError(f"{source}: expected pinned SHA-256 {digest}, got {actual}")
        inputs.append((name, source, modes))
    output.mkdir(parents=True, exist_ok=True)
    julia = ["julia", f"--project={HERE}"]
    run([*julia, HERE / "test_checks.jl"])
    run([*julia, HERE / "test_solve_checks.jl"])
    summary = {}
    for name, source, modes in inputs:
        directory = output / name
        run(["cargo", "run", "--quiet", "-p", "powerio", "--example", "acdc_validation", "--", source, directory])
        run([*julia, HERE / "compare.jl", source, directory])
        run([*julia, HERE / "solve.jl", source, directory, modes])
        results = json.loads((directory / "solve.json").read_text())
        summary[name] = {
            "source_sha256": results["source_sha256"],
            "fresh_sha256": results["fresh_sha256"],
            "whole_case": "passed",
            "solves": {
                kind: {
                    "objective": result["source"]["objective"],
                    "max_state_difference": result["max_absolute_result_difference"],
                    "max_primal_violation": max(result[label]["max_primal_violation"] for label in (
                        "source", "fresh", "source_point_in_fresh_model", "fresh_point_in_source_model")),
                }
                for kind, result in results["solves"].items()
            },
        }
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"PASS: {len(inputs)} cases; results in {output / 'summary.json'}")


if __name__ == "__main__":
    main()

"""Qualify source validity at construction and primitive-building boundaries.

The synthetic cases include disabled sources, singular coupled impedances,
small valid impedances and repairs before the first solve. Native failures
are observations, never usable networks with substituted fallback resistance.
"""

import argparse
import hashlib
import json
from pathlib import Path

import opendssdirect as dss
from oracle import LOCK


def profiles():
    # Expected native error: zero Z1 is rejected during construction (7340),
    # whereas a singular primitive is rejected at the first solve (325).
    values = {
        "zero_z1": ("phases=1 z1=[0 0]", 7340),
        "zero_ideal_one": ("phases=1 model=ideal puzideal=[0 0]", 325),
        "zero_ideal_three": ("phases=3 model=ideal puzideal=[0 0]", 325),
        "zero_sequence": ("phases=3 z1=[1 0] z0=[0 0]", 325),
        "singular_two": ("phases=2 z1=[1 0] z0=[-0.5 0]", 325),
        "singular_four": ("phases=4 z1=[1 0] z0=[0.25 0]", 325),
        "tiny_z1": ("phases=1 z1=[1e-12 0]", None),
        "tiny_complex": ("phases=1 z1=[1e-12 2e-12]", None),
        "tiny_ideal_one": ("phases=1 model=ideal puzideal=[1e-12 0]", None),
        "tiny_ideal_three": ("phases=3 model=ideal puzideal=[1e-12 0]", None),
        "near_zero_sequence": ("phases=3 z1=[1 0] z0=[1e-6 0]", None),
        "near_singular_two": ("phases=2 z1=[1 0] z0=[-0.499999 0]", None),
        "near_singular_four": ("phases=4 z1=[1 0] z0=[0.250001 0]", None),
        "same_edit_repair": ("phases=1 z1=[0 0] z1=[1 0]", None),
        "late_constructor_repair": (
            "phases=1 z1=[0 0]\nEdit Vsource.aux z1=[1 0]",
            7340,
        ),
        "late_edit_repair": (
            "phases=1 z1=[1 0]\nEdit Vsource.aux z1=[0 0]\nEdit Vsource.aux z1=[1 0]",
            7340,
        ),
        "primitive_repaired_before_solve": (
            "phases=3 z1=[1 0] z0=[0 0]\nEdit Vsource.aux z0=[1 0]",
            None,
        ),
        "primitive_invalidated_by_edit": (
            "phases=3 z1=[1 0]\nEdit Vsource.aux z0=[0 0]",
            325,
        ),
        "ideal_repaired_before_solve": (
            "phases=1 model=ideal puzideal=[0 0]\n"
            "Edit Vsource.aux puzideal=[0.01 0.02]",
            None,
        ),
    }
    return values


def observe(script):
    engine = dss.NewContext()
    version = engine.Basic.Version()
    if LOCK["dss"]["version"] not in version:
        raise ValueError("unpinned native engine")
    commands = []
    error = None
    for command in script.splitlines():
        try:
            engine.Text.Command(command)
            commands.append({"command": command, "accepted": True})
        except Exception as exc:
            error = str(exc)
            commands.append({"command": command, "error": error})
            break
    converged = engine.Solution.Converged() if error is None else False
    return {
        "engine": version,
        "commands": commands,
        "error": error,
        "converged": converged,
        "accepted": error is None and converged,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    cases = {}
    failures = []
    for label, (properties, expected_error) in profiles().items():
        for frequency in (50, 60):
            for enabled in ("yes", "no"):
                name = f"{label}/{frequency}/{enabled}"
                # An independently energized, loaded circuit prevents a
                # de-energized convergence failure from masking validity.
                script = (
                    f"Clear\nSet DefaultBaseFrequency={frequency}\n"
                    "New Circuit.c phases=3 bus1=feed basekv=0.24 z1=[1 0]\n"
                    "New Load.test phases=3 bus1=feed kv=0.24 kw=1 model=2\n"
                    f"New Vsource.aux bus1=remote basekv=0.24 enabled={enabled} "
                    f"{properties}\n"
                    "New Reactor.termination phases=4 bus1=remote r=10 x=0\n"
                    "Set controlmode=off\nSolve\n"
                )
                result = observe(script)
                result.update(
                    script=script, sha256=hashlib.sha256(script.encode()).hexdigest()
                )
                result["expected_error"] = expected_error
                if expected_error is None:
                    if not result["accepted"]:
                        failures.append(name)
                elif f"#{expected_error}" not in (result["error"] or ""):
                    failures.append(name)
                cases[name] = result
    args.output.mkdir(parents=True, exist_ok=True)
    report = {"cases": cases, "failures": failures}
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"cases": len(cases), "failures": failures}))
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()

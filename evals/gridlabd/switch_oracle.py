"""Qualify native static switch impedance before implementing PowerIO mappings.

This is engine calibration, not a claim of GLM switch conversion support.
The two pinned implementations use solver-dependent defaults and replace zero
NR impedance components. Explicit module defaults make the reference independent
of compiled global defaults. No controls or protection behavior is executed.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

import numpy as np

from oracle import add, analytic, cases, compare, glm, recorded, run_glm


def variants():
    return [
        ("nr_default", "NR", "", complex(0.0001, -0.0001)),
        ("nr_zero", "NR", "switch_impedance 0+0j;", complex(0.0001, 0.0001)),
        ("nr_given", "NR", "switch_impedance 1+0.5j;", complex(1, 0.5)),
        ("fbs_default", "FBS", "", 0j),
        ("fbs_given", "FBS", "switch_impedance 1+0.5j;", complex(1, 0.5)),
    ]


def snapshot(directory):
    result = {}
    for bus in ("source", "sink"):
        add(
            result,
            f"{bus}.A.voltage",
            "V",
            recorded(directory / f"{bus}.csv")["voltage_A"],
        )
    values = recorded(directory / "line.csv")
    for side, suffix, sign in (("from", "in", 1), ("to", "out", -1)):
        for quantity, unit in (("current", "A"), ("power", "VA")):
            add(
                result,
                f"line.{side}.A.{quantity}",
                unit,
                sign * values[f"{quantity}_{suffix}_A"],
            )
    add(result, "line.loss", "VA", values["power_losses"])
    return result


def reference(source, loads, impedance):
    result = analytic(source, np.array([[impedance]]), loads)
    # Switch impedance is independently inferred from solved terminal values,
    # not read from input properties or the line-only impedance dump adapter.
    del result["line.z.AA"]
    return result


def run_case(profile, directory, pnnl_root, solver, properties, expected_z):
    source, line_z, loads = cases()["single_phase"]
    deck = glm(source, line_z, loads).replace(
        "solver_method NR;", f"solver_method {solver}; default_resistance 0.0001;"
    )
    deck, count = re.subn(
        r"object overhead_line \{[^}]*\}",
        "object switch { name line; phases AN; from source; to sink; "
        + properties
        + " }",
        deck,
    )
    if count != 1:
        raise ValueError("expected exactly one series link in the analytic fixture")
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "model.glm").write_text(deck)
    version = run_glm(profile, directory, pnnl_root)
    actual = snapshot(directory)
    voltage = lambda key: complex(*actual[key]["value"])
    current = voltage("line.from.A.current")
    if abs(current) < 1.0:
        raise ValueError("insufficient current for an impedance witness")
    derived = (voltage("source.A.voltage") - voltage("sink.A.voltage")) / current
    comparison = compare(reference(source, loads, expected_z), actual)
    ideal = compare(reference(source, loads, 0j), actual)
    impedance_passed = abs(derived - expected_z) < 1e-10 + 1e-9 * abs(expected_z)
    negative_control_passed = expected_z == 0 or not ideal["passed"]
    return {
        "passed": comparison["passed"] and impedance_passed and negative_control_passed,
        "version": version,
        "input_sha256": hashlib.sha256(deck.encode()).hexdigest(),
        "electrical_comparison": comparison,
        "derived_impedance": [derived.real, derived.imag],
        "expected_impedance": [expected_z.real, expected_z.imag],
        "impedance_passed": impedance_passed,
        "ideal_comparison": ideal,
        "ideal_negative_control_required": expected_z != 0,
        "ideal_negative_control_passed": negative_control_passed,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path)
    parser.add_argument(
        "--profiles", nargs="+", choices=("pnnl", "arras"), default=["pnnl", "arras"]
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = {"scope": __doc__, "cases": {}}
    for profile in args.profiles:
        for name, solver, properties, impedance in variants():
            key = f"{profile}/{name}"
            try:
                result = run_case(
                    profile,
                    args.output / profile / name,
                    args.pnnl_root,
                    solver,
                    properties,
                    impedance,
                )
            except Exception as error:
                result = {"passed": False, "error": str(error)}
            report["cases"][key] = result
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
            print(f"{key}: {'passed' if result['passed'] else 'FAILED'}", flush=True)
    return 0 if all(case["passed"] for case in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())

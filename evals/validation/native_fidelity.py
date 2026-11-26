"""Check file recovery, parsed grid data, and electrical results on two reference cases."""

import argparse
import hashlib
import json
import math
import os
import subprocess
import sys
import tempfile
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
IEEE13 = "tests/data/dist/opendss/ieee13/"
CASES = {
    "case14": {
        "source": "matpower",
        "target": "matpower",
        "fixture": "tests/data/case14.m",
        "provenance": "tests/data/README.md#matpower-cases-m-in-this-directory",
        "digests": {
            "tests/data/case14.m": "2ffc4e1b734ae6c5e92dbe68b4e36010ed695a4bbcc4d065c74c4fbc39fcf3c1",
        },
        "outside_scope": "Independent AC power flow, limits, and OPF costs.",
    },
    "ieee13": {
        "source": "dss",
        "target": "dss",
        "fixture": IEEE13 + "IEEE13Nodeckt.dss",
        "provenance": "tests/data/dist/README.md#opendss",
        "digests": {
            IEEE13 + "IEEE13Nodeckt.dss": "033ccd4005c6538e9de5b2218e83001802ce14fbc5b6c312896ae8cbc5d4a504",
            IEEE13 + "IEEELineCodes.DSS": "71bc16ef9c6084b9e3bab74fbdf967080b79624683465e1483b787ff160f58b3",
            IEEE13 + "IEEE13Node_BusXY.csv": "f8dd1528c72f339ebf41bf1355ed64ef12fbe30864fcc839ed5e5e9dc527846e",
        },
        "outside_scope": "Independent voltage angles, branch powers, control trajectories, and coordinates.",
    },
}
TEST_LABELS = {
    "byte_replay": "Copy the original files unchanged",
    "fresh_write": "Write new files and check them in PowerIO",
    "independent_validation": "Compare original and new files using external tools",
}
LEGS = tuple(TEST_LABELS)
METRIC_LABELS = {
    "matpower_dc_bbus": "Bus susceptance matrix (Bbus)",
    "matpower_dc_bf": "Branch flow matrix (Bf)",
    "matpower_dc_pbusinj": "Bus phase-shift injections",
    "matpower_dc_pfinj": "Branch phase-shift injections",
    "matpower_dc_ptdf": "Power transfer distribution factors",
    "opendss_voltage_magnitude": "Voltage magnitude",
}
COLOURS = {"pass": "🟢", "fail": "🔴", "missing": "🟡"}
B_BUS_CHECK = "matpower_dc_bbus"
DC_TOL = 1e-9
VOLTAGE_TOL = 1e-3


def check_spec(case):
    """Fixed applicability. A failed prerequisite leaves its dependents missing."""
    checks = {
        "retained_files": (LEGS[0], "Exact inventory and bytes of all original files."),
        "source_parse": (LEGS[1], "PowerIO parses the original native fixture."),
        "fresh_write": (LEGS[1], "Emit from the typed network with no retained bytes."),
        "powerio_readback": (LEGS[1], "PowerIO parses the fresh output and sidecars."),
        "matrix_invariants": (
            LEGS[1],
            (
                "Matrix core counts and totals, AC Y_bus and connected bus injections "
                "with normalized tolerance 1e-8."
                if case == "case14" else
                "Matrix core counts/totals, source constraints (1e-9 × max(|emf|, 1)), "
                "and line, switch, injection, shunt and transformer electrical projections "
                "(1e-9 × max(|before|, |after|, 1e-12))."
            ),
        ),
    }
    if case == "case14":
        checks["matpower_axes"] = (
            LEGS[2], "CaseFrames bus IDs, bus types, oriented branch endpoints and service flags agree in input order."
        )
        for metric in ("bbus", "bf", "pbusinj", "pfinj", "ptdf"):
            checks[f"matpower_dc_{metric}"] = (
                LEGS[2], f"Maximum entrywise absolute residual for MATPOWER's {metric} (including zeros)."
            )
    else:
        checks["opendss_nodes"] = (LEGS[2], "Native solves converge and their labelled conductor sets agree.")
        checks["opendss_voltage_magnitude"] = (
            LEGS[2], "Maximum absolute voltage-magnitude residual across all labelled conductors."
        )
    spec = {name: {"leg": leg, "scope": scope} for name, (leg, scope) in checks.items()}
    for name in spec:
        if name.startswith("matpower_dc_"):
            spec[name].update(tolerance=DC_TOL, units="unitless" if name.endswith("_ptdf") else "per unit", comparison="<")
        if name == "opendss_voltage_magnitude":
            spec[name].update(tolerance=VOLTAGE_TOL, units="V", comparison="<=")
    return spec


def fixture(case):
    return ROOT / CASES[case]["fixture"]


def verify_fixtures():
    for case in CASES.values():
        for file, expected in case["digests"].items():
            actual = hashlib.sha256((ROOT / file).read_bytes()).hexdigest()
            if actual != expected:
                raise ValueError(f"{file}: fixture SHA-256 changed ({expected} -> {actual})")


def outcome(passed, **details):
    return {"status": "pass" if passed else "fail", **details}


def numerical(residual, rule, coverage, **detail):
    finite = math.isfinite(residual)
    tolerance = rule["tolerance"]
    passed = finite and (residual <= tolerance if rule["comparison"] == "<=" else residual < tolerance)
    return outcome(
        passed, residual=residual if finite else None, coverage=coverage,
        **(detail if finite else {**detail, "detail": "non-finite residual"}),
    )


def matpower_checks(source, target):
    import numpy as np
    from validate_dc_matpower import ABS_TOL, CaseFrames, matpower_dc, matpower_ptdf, matrix_difference

    if ABS_TOL != DC_TOL:
        raise ValueError("MATPOWER oracle tolerance differs from the benchmark spec")
    rules = check_spec("case14")
    reference = matpower_dc(CaseFrames(source))
    try:
        candidate = matpower_dc(CaseFrames(target))
    except (ValueError, RuntimeError, KeyError, IndexError) as error:
        return {"matpower_axes": outcome(False, detail=f"Fresh output rejected: {error}")}
    axis_diffs = [
        axis for axis in ("bus_i", "bus_type", "f", "t", "keep")
        if not np.array_equal(reference[axis], candidate[axis])
    ]
    results = {
        "matpower_axes": outcome(
            not axis_diffs, differing_axes=axis_diffs,
            coverage={"buses": reference["nb"], "branches": len(reference["f"])},
        )
    }
    if axis_diffs:
        return results
    for metric in ("Bbus", "Bf", "Pbusinj", "Pfinj"):
        a, b = reference[metric], candidate[metric]
        if metric in ("Bbus", "Bf"):
            residual = matrix_difference(a, b)
        else:
            residual = float(np.max(np.abs(a - b)))
        check = f"matpower_dc_{metric.lower()}"
        results[check] = numerical(
            residual, rules[check], {"shape": list(a.shape), "entries": math.prod(a.shape)},
        )
    expected = matpower_ptdf(reference)
    try:
        actual = matpower_ptdf(candidate)
    except np.linalg.LinAlgError as error:
        results["matpower_dc_ptdf"] = outcome(False, detail=f"Fresh output PTDF solve failed: {error}")
    else:
        results["matpower_dc_ptdf"] = numerical(
            float(np.max(np.abs(expected - actual))), rules["matpower_dc_ptdf"],
            {"shape": list(expected.shape), "entries": expected.size},
        )
    return results


def opendss_checks(source, target):
    from validate_opendss import TOLERANCE_VOLTS, solve_voltage_magnitudes

    if TOLERANCE_VOLTS != VOLTAGE_TOL:
        raise ValueError("OpenDSS oracle tolerance differs from the benchmark spec")
    # OpenDSS Redirect may change the process working directory.
    cwd = Path.cwd()
    try:
        expected = solve_voltage_magnitudes(source)
        try:
            actual = solve_voltage_magnitudes(target)
        except Exception as error:
            return {"opendss_nodes": outcome(False, detail=f"Fresh deck solve failed: {error}")}
    finally:
        os.chdir(cwd)
    missing = sorted(expected.keys() - actual.keys())
    extra = sorted(actual.keys() - expected.keys())
    common = expected.keys() & actual.keys()
    coverage = {"reference": len(expected), "candidate": len(actual), "common": len(common)}
    results = {
        "opendss_nodes": outcome(
            bool(common) and not (missing or extra), missing_nodes=missing,
            extra_nodes=extra, coverage=coverage,
        ),
    }
    if results["opendss_nodes"]["status"] == "pass":
        residuals = {node: abs(expected[node] - actual[node]) for node in sorted(common)}
        worst = max(residuals, key=lambda node: (
            not math.isfinite(residuals[node]), residuals[node]
        ))
        results["opendss_voltage_magnitude"] = numerical(
            residuals[worst], check_spec("ieee13")["opendss_voltage_magnitude"], coverage,
            worst_node=worst,
            reference_volts=expected[worst] if math.isfinite(expected[worst]) else None,
            candidate_volts=actual[worst] if math.isfinite(actual[worst]) else None,
        )
    return results


def write_reactance_error(case, output):
    """Double the series reactance of the case14 branch from bus 1 to bus 2."""
    text = case.read_text()
    header, table = text.split("mpc.branch = [", 1)
    branch_table, end = table.split("];", 1)
    lines = branch_table.splitlines(keepends=True)
    rows = [i for i, line in enumerate(lines) if line.split()[:2] == ["1", "2"]]
    if len(rows) != 1:
        raise ValueError("expected exactly one branch from bus 1 to bus 2")
    row = rows[0]
    fields = lines[row].split()
    original = float(fields[3])
    fields[3] = str(2 * original)
    lines[row] = "\t" + "\t".join(fields) + "\n"
    # Octave uses the first function in the file. Keep its name aligned with
    # the file name when the copy is read by MATPOWER.
    if case.stem != output.stem:
        header = header.replace(f"mpc = {case.stem}", f"mpc = {output.stem}", 1)
    output.write_text(header + "mpc.branch = [" + "".join(lines) + "];" + end)
    return original


def negative_control(fresh, normal):
    corrupt = fresh.with_name("case14_mutated.m")
    original = write_reactance_error(fresh, corrupt)
    result = matpower_checks(fixture("case14"), corrupt).get(B_BUS_CHECK, {"status": "missing"})
    detected = (
        normal[B_BUS_CHECK]["status"] == "pass"
        and result["status"] == "fail" and result.get("residual") is not None
    )
    rule = check_spec("case14")[B_BUS_CHECK]
    return outcome(
        detected, check=B_BUS_CHECK, branch=[1, 2], field="BR_X (pu)",
        before=original, after=2 * original, result={**rule, **result},
    )


def oracle_versions():
    packages = (
        "matpowercaseframes", "numpy", "pandas", "scipy",
        "opendssdirect.py", "dss-python", "dss-python-backend",
    )
    versions = {}
    for package in packages:
        try:
            versions[package] = version(package)
        except PackageNotFoundError:
            versions[package] = None
    try:
        from opendssdirect import dss
        versions["engine"] = dss.Basic.Version().splitlines()[0].strip()
    except (ImportError, OSError):
        versions["engine"] = None
    return versions


def score(checks):
    return {
        leg: {
            "passes": sum(c["status"] == "pass" for c in checks.values() if c["leg"] == leg),
            "verified": sum(c["status"] != "missing" for c in checks.values() if c["leg"] == leg),
            "applicable": sum(c["leg"] == leg for c in checks.values()),
        }
        for leg in LEGS
    }


def run(directory, skip_independent, compact):
    subprocess.run(
        ["cargo", "test", "--locked", "-p", "powerio-cli", "--test", "conversion_matrix_report",
         "native::write_native_seed_evidence", "--", "--ignored", "--exact"],
        cwd=ROOT, env={**os.environ, "POWERIO_NATIVE_FIDELITY_DIR": str(directory)}, check=True,
    )
    matrix = json.loads((directory / "matrix.json").read_text())
    report = {
        "schema": "powerio-native-fidelity",
        "schema_version": 1,
        "spec": "native-seeds-v1",
        "provenance": matrix["provenance"],
        "powerio_version": matrix["powerio_version"],
        "oracles": oracle_versions(),
        "cases": {},
        "negative_control": {"status": "missing"},
    }
    for name, spec in CASES.items():
        evidence = matrix["measurements"][name]
        observed = evidence.pop("checks")
        missing_reason = "Prerequisite check did not pass."
        if skip_independent:
            missing_reason = "Independent checks disabled for this run."
        elif observed.get("fresh_write", {}).get("status") == "pass":
            try:
                if name == "case14":
                    fresh = directory / "case14.m"
                    native = matpower_checks(fixture(name), fresh)
                else:
                    native = opendss_checks(fixture(name), directory / "ieee13/input.dss")
                observed.update(native)
            except Exception as error:
                missing_reason = f"Independent oracle unavailable: {error}"
        if name == "case14" and observed.get(B_BUS_CHECK, {}).get("status") == "pass":
            try:
                report["negative_control"] = negative_control(fresh, observed)
            except ValueError as error:
                report["negative_control"] = outcome(False, detail=str(error))
        checks = {
            key: {
                **definition,
                **observed.get(key, {
                    "status": "missing",
                    "detail": missing_reason if definition["leg"] == LEGS[2]
                    else "Prerequisite check did not pass.",
                }),
            }
            for key, definition in check_spec(name).items()
        }
        if observed.keys() - checks.keys():
            raise ValueError(f"Unexpected applicable checks on {name}: {observed.keys() - checks.keys()}")
        report["cases"][name] = {**spec, "score": score(checks), "checks": checks}
        if not compact:
            report["cases"][name]["matrix_evidence"] = evidence
    return report


def print_scorecard(report):
    print("\n| Input | Output format | What the test does | Checks passed | Checks run |")
    print("| --- | --- | --- | ---: | ---: |")
    for name, case in report["cases"].items():
        for leg, counts in case["score"].items():
            p, v, a = (counts[k] for k in ("passes", "verified", "applicable"))
            status = "fail" if p < v else "missing" if v < a else "pass"
            print(f"| {name} ({case['source']}) | {case['target']} | {TEST_LABELS[leg]} "
                  f"| {COLOURS[status]} {p}/{a} | {v}/{a} |")
    print()
    for name, case in report["cases"].items():
        for key, check in case["checks"].items():
            if "residual" in check:
                label = f"{COLOURS[check['status']]} {name} — {METRIC_LABELS[key]}"
                if check["residual"] is None:
                    print(f"{label} produced a non-finite difference.")
                    continue
                location = f" at node {check['worst_node']}" if "worst_node" in check else ""
                print(f"{label}. Largest difference "
                      f"{check['residual']:.6g} {check['units']}{location} "
                      f"(required {check['comparison']} {check['tolerance']:.6g} {check['units']}).")
                if check.get("reference_volts") is not None and check.get("candidate_volts") is not None:
                    print(f"   Original {check['reference_volts']:.3f} V, new {check['candidate_volts']:.3f} V.")
            elif check["status"] != "pass":
                detail = check.get("detail", check["scope"])
                print(f"{COLOURS[check['status']]} {name}/{key} {check['status']}. {detail}")
    control = report["negative_control"]
    if "result" in control:
        result = control["result"]
        print(f"For the deliberate error test, the script changed case14 branch 1–2 reactance "
              f"from {control['before']} to {control['after']} per unit.")
        print(f"{control['check']} returned {result['status']}.")
        if result.get("residual") is not None:
            print(f"The matrix differed by {result['residual']:.6g} per unit.")
    elif "detail" in control:
        print(f"The deliberate error test failed. {control['detail']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--compact", action="store_true", help="omit full matrix diagnostic drill-down")
    parser.add_argument("--skip-independent", action="store_true", help="record independent checks as missing")
    args = parser.parse_args()
    verify_fixtures()
    with tempfile.TemporaryDirectory(prefix="powerio-native-fidelity-") as tmp:
        report = run(Path(tmp), args.skip_independent, args.compact)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print_scorecard(report)
    missing = any(
        check["status"] == "missing"
        for case in report["cases"].values()
        for check in case["checks"].values()
        if not (args.skip_independent and check["leg"] == LEGS[2])
    )
    if missing or report["negative_control"]["status"] == "fail":
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

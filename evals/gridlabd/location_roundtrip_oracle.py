"""Validate typed/IR/canonical/edited coordinates and unchanged circuit physics.

Native saves independently expose object coordinates. The pinned engines replace
a line's declared location with the endpoint midpoint during initialization;
that derived state is checked separately from the source-declared typed point.
No datum is inferred, partial axes remain absent and geometry never sets length.
"""

import argparse
import copy
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from coordinate_oracle import locations
from network_oracle import residuals
from oracle import analytic, cases, command, compare, glm, read_glm, run_glm
from roundtrip_oracle import equivalent, instrumentation, snapshot


def check_points(actual, expected, tolerance):
    if actual.keys() != expected.keys():
        raise ValueError("coordinate object inventory differs")
    for name, point in expected.items():
        if point.keys() != actual[name].keys():
            raise ValueError(f"coordinate presence differs: {name}")
        for axis, value in point.items():
            if not math.isclose(
                actual[name][axis], value, rel_tol=0, abs_tol=tolerance
            ):
                raise ValueError(f"coordinate mismatch: {name}.{axis}")


def declared(records):
    result = {}
    for record in records:
        key = record["target"] + "/" + record["name"]
        if key in result or record["point"]["datum"] is not None:
            raise ValueError("duplicate attachment or invented datum")
        result[key] = {
            axis: record["point"][axis]
            for axis in ("latitude", "longitude")
            if record["point"][axis] is not None
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    source, impedance, load = cases()["single_phase"]
    text = (
        glm(source, impedance, load)
        .replace("name source;", "name source; latitude 27S28:12; longitude 153E1:12;")
        .replace("name sink;", "name sink; latitude -27.5; longitude 154;")
        .replace("name cfg;", "name cfg; latitude 0;")
        .replace("name line;", "name line; latitude 10; longitude 20;")
    )
    text += "object load { name demand; parent sink; phases AN; nominal_voltage 240; constant_impedance_A 50+10j; latitude 0; longitude (source.longitude); }\n"
    expected = {
        "bus/source": {"latitude": -27.47, "longitude": 153.02},
        "bus/sink": {"latitude": -27.5, "longitude": 154.0},
        "line/line": {"latitude": 10.0, "longitude": 20.0},
        "line_code/line__glm_linecode": {"latitude": 0.0},
        "load/demand": {"latitude": 0.0, "longitude": 153.02},
    }
    report = {
        "scope": __doc__,
        "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
        "cases": {},
    }
    outputs = ("source.csv", "sink.csv", "line.csv", "impedance.xml", "coordinates.glm")
    parallel = np.array([1 / (1 / load[0] + 1 / (50 + 10j))])
    for profile in ("pnnl", "arras"):
        directory = args.output.resolve() / profile
        original = directory / "original"
        original.mkdir(parents=True)
        (original / "model.glm").write_text(text)
        parsed = json.loads(
            command(
                [
                    str(args.bridge.resolve()),
                    str(original / "model.glm"),
                    str(directory),
                    "gridlabd" if profile == "pnnl" else "gridlabd-arras",
                ],
                directory,
            )
        )
        if (directory / "fresh/case.glm").read_bytes() != (
            directory / "restored/case.glm"
        ).read_bytes():
            raise ValueError("IR changed canonical coordinate output")
        (original / "model.glm").write_text("#set savefile=coordinates.glm\n" + text)
        baseline_engine = run_glm(profile, original, args.pnnl_root.resolve(), outputs)
        baseline = snapshot(original)
        for variant, data in parsed.items():
            wanted = copy.deepcopy(expected)
            if variant == "edited":
                for point in wanted.values():
                    for axis in point:
                        point[axis] += 1 if axis == "latitude" else -1
            elif variant == "unlocated":
                wanted = {}
            check_points(declared(data["locations"]), wanted, 1e-12)
            check_points(declared(data["reparsed_locations"]), wanted, 1e-12)
            if data["network"] != parsed["unlocated"]["network"]:
                raise ValueError("coordinate operation changed electrical network")
            target = directory / variant
            canonical = (target / "case.glm").read_text()
            (target / "model.glm").write_text(
                "#set savefile=coordinates.glm\n" + canonical + instrumentation("A")
            )
            engine = run_glm(profile, target, args.pnnl_root.resolve(), outputs)
            native_expected = {
                "source": wanted.get("bus/source", {}),
                "sink": wanted.get("bus/sink", {}),
                "demand": wanted.get("load/demand", {}),
                "line__glm_configuration": wanted.get(
                    "line_code/line__glm_linecode", {}
                ),
                "line": {},
            }
            if wanted:
                native_expected["line"] = {
                    axis: (wanted["bus/source"][axis] + wanted["bus/sink"][axis]) / 2
                    for axis in ("latitude", "longitude")
                }
            actual = locations(target / "coordinates.glm", set(native_expected))
            check_points(actual, native_expected, 1.4e-6)
            checks = residuals(data["network"], target, "A")
            equality = equivalent(snapshot(target), baseline)
            independent = compare(
                analytic(source, impedance, parallel), read_glm(target, 1)
            )
            if not independent["passed"]:
                raise ValueError("independent circuit solution differs")
            negatives = []
            if wanted:
                for mutation in ("axis", "identity"):
                    wrong = copy.deepcopy(actual)
                    if mutation == "axis":
                        wrong["source"]["latitude"] = wrong["source"]["longitude"]
                    else:
                        wrong["source"], wrong["demand"] = (
                            wrong["demand"],
                            wrong["source"],
                        )
                    try:
                        check_points(wrong, native_expected, 1.4e-6)
                    except ValueError:
                        negatives.append(mutation)
                    else:
                        raise ValueError(f"coordinate comparator missed {mutation}")
            report["cases"][f"{profile}/{variant}"] = {
                "engine": engine,
                "baseline_engine": baseline_engine,
                "bridge": data,
                "native_points": actual,
                "electrical_residuals": checks,
                "baseline_equivalence": equality,
                "independent_linear_solve": independent,
                "negative_controls": negatives,
                "passed": True,
            }
            print(
                profile, variant, "PASS", len(actual), "native point states", flush=True
            )
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

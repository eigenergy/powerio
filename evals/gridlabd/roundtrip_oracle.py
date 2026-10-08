"""Check source-free canonical GLM, IR-restored, edited and equivalent networks.

The native input is regenerated exclusively from typed network values. Recorder
and clock instrumentation is added independently by this harness. Original and
canonical native results must agree; edited output must satisfy the edited
network's equations and produce a measurably different result.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np

from network_oracle import decks, residuals
from oracle import analytic, command, compare, read_glm, recorded, run_glm


def instrumentation(phases):
    text = """
#set complex_format=%.15g%+.15g%c
#set complex_output_format=DEFAULT
module tape;
clock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:02'; }
object impedance_dump { filename "impedance.xml"; }
"""
    for bus in ("source", "sink"):
        props = ",".join(f"voltage_{p}" for p in phases)
        text += f'object recorder {{ parent {bus}; property {props}; interval 1; file "{bus}.csv"; }}\n'
    props = [
        f"{kind}_{p}"
        for p in phases
        for kind in ("current_in", "current_out", "power_in", "power_out")
    ]
    text += f'object recorder {{ parent line; property {",".join(props)},power_losses; interval 1; file "line.csv"; }}\n'
    return text


def snapshot(directory):
    return {
        f"{object_name}.{key}": value
        for object_name in ("source", "sink", "line")
        for key, value in recorded(directory / f"{object_name}.csv").items()
    }


def equivalent(actual, expected):
    if actual.keys() != expected.keys():
        raise ValueError("canonical output changed the measured quantity inventory")
    errors = {}
    for key, value in expected.items():
        error = abs(actual[key] - value)
        bound = 2e-6 + 2e-8 * abs(value)
        if not error <= bound:
            raise ValueError(
                f"{key}: canonical result differs by {error}, bound {bound}"
            )
        errors[key] = float(error)
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}, "standalone": {}}
    outputs = ("source.csv", "sink.csv", "line.csv", "impedance.xml")
    for profile in ("pnnl", "arras"):
        for name, text, phases, linear_case in decks():
            directory = args.output.resolve() / profile / name
            original = directory / "original"
            original.mkdir(parents=True)
            (original / "model.glm").write_text(text)
            parsed = json.loads(
                command(
                    [
                        str(args.bridge.resolve()),
                        str(original / "model.glm"),
                        str(directory),
                        profile,
                    ],
                    directory,
                )
            )
            if (directory / "fresh.glm").read_bytes() != (
                directory / "restored.glm"
            ).read_bytes():
                raise ValueError("IR changed canonical output")
            standalone = directory / "standalone"
            standalone.mkdir()
            (standalone / "model.glm").write_bytes(
                (directory / "fresh.glm").read_bytes()
            )
            report["standalone"][f"{profile}/{name}"] = run_glm(
                profile, standalone, args.pnnl_root.resolve(), ()
            )
            baseline_version = run_glm(
                profile, original, args.pnnl_root.resolve(), outputs
            )
            baseline = snapshot(original)
            for variant, data in parsed.items():
                target = directory / variant
                target.mkdir()
                canonical = (directory / f"{variant}.glm").read_text()
                model = canonical + instrumentation(phases)
                (target / "model.glm").write_text(model)
                version = run_glm(profile, target, args.pnnl_root.resolve(), outputs)
                checks = residuals(data["network"], target, phases)
                actual = snapshot(target)
                equivalence = None
                if variant == "edited":
                    if max(abs(actual[key] - baseline[key]) for key in baseline) < 1e-3:
                        raise ValueError(
                            "edited network produced the unchanged source result"
                        )
                else:
                    equivalence = equivalent(actual, baseline)
                independent = None
                if linear_case is not None:
                    source, impedance, loads = [
                        np.array(value, copy=True) for value in linear_case
                    ]
                    if variant == "edited":
                        source[0] *= 1.03
                        impedance *= 1.1
                        loads /= 1.25
                    independent = compare(
                        analytic(source, impedance, loads),
                        read_glm(target, len(phases)),
                    )
                    if not independent["passed"]:
                        raise ValueError(
                            "canonical output disagrees with independent linear solution"
                        )
                report["cases"][f"{profile}/{name}/{variant}"] = {
                    "engine": version,
                    "baseline_engine": baseline_version,
                    "canonical_sha256": hashlib.sha256(canonical.encode()).hexdigest(),
                    "instrumented_sha256": hashlib.sha256(model.encode()).hexdigest(),
                    "network": data["network"],
                    "diagnostics": data["diagnostics"],
                    "electrical_residuals": checks,
                    "equivalence_errors": equivalence,
                    "independent_linear_solve": independent,
                    "passed": True,
                }
                print(profile, name, variant, "PASS", flush=True)
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

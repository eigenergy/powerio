"""Qualify facade profiles, multi-file replay, IR and edited/cross-profile output.

A conditional in an included file deliberately selects different impedances
under the pinned PNNL and Arras preprocessors. Every output is solved by its
declared target engine and compared with both the typed equations and an
independent linear solution. Source replay is counted separately from canonical
generation, and canonical cases receive independent recorder instrumentation.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

import numpy as np
from network_oracle import residuals
from oracle import analytic, cases, command, compare, glm, read_glm, run_glm
from roundtrip_oracle import equivalent, instrumentation, snapshot


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    baseline_snapshots = {}
    source, impedance, loads = cases()["single_phase"]
    text = glm(source, impedance, loads)
    matches = list(re.finditer(r"object load \{[^{}]*\}", text))
    if len(matches) != 1 or "20+5j" not in matches[0][0]:
        raise ValueError("microcase template changed")
    obj = matches[0][0]
    main_text = (
        "#define SELECT=100\n"
        + text[: matches[0].start()]
        + '#include "parts/load.glm"\n'
        + text[matches[0].end() :]
    )
    include = f"#if SELECT<20\n{obj}\n#else\n{obj.replace('20+5j', '40+5j')}\n#endif\n"
    outputs = ("source.csv", "sink.csv", "line.csv", "impedance.xml")
    for profile in ("pnnl", "arras"):
        directory = args.output.resolve() / profile
        original = directory / "original"
        (original / "parts").mkdir(parents=True)
        files = {"model.glm": main_text.encode(), "parts/load.glm": include.encode()}
        for name, data in files.items():
            (original / name).write_bytes(data)
        token = "gridlabd" if profile == "pnnl" else "gridlabd-arras"
        parsed = json.loads(
            command(
                [
                    str(args.bridge.resolve()),
                    str(original / "model.glm"),
                    str(directory),
                    token,
                ],
                directory,
            )
        )
        for name, data in files.items():
            if (directory / "replay" / name).read_bytes() != data:
                raise ValueError(f"source replay changed {name}")
        baseline_engine = run_glm(profile, original, args.pnnl_root.resolve(), outputs)
        baseline = snapshot(original)
        baseline_snapshots[profile] = baseline
        for variant, data in parsed.items():
            target = directory / variant
            expected_fidelity = (
                "ExactSameFormat" if variant == "replay" else "Canonical"
            )
            if data["fidelity"] != expected_fidelity:
                raise ValueError(f"incorrect fidelity for {variant}")
            if variant != "replay":
                (target / "model.glm").write_text(
                    (target / "case.glm").read_text() + instrumentation("A")
                )
            engine_profile = "pnnl" if data["target"] == "gridlabd" else "arras"
            engine = run_glm(engine_profile, target, args.pnnl_root.resolve(), outputs)
            checks = residuals(data["network"], target, "A")
            actual = snapshot(target)
            equality = None
            if variant == "edited":
                if max(abs(actual[k] - baseline[k]) for k in baseline) < 1e-3:
                    raise ValueError("edited output did not change electrical results")
            else:
                equality = equivalent(actual, baseline)
            expected_loads = np.array(loads, copy=True)
            expected_loads[0] = 20 + 5j if profile == "pnnl" else 40 + 5j
            if variant == "edited":
                expected_loads /= 1.25
            independent = compare(
                analytic(source, impedance, expected_loads), read_glm(target, 1)
            )
            if not independent["passed"]:
                raise ValueError(f"{profile}/{variant}: analytic solution disagrees")
            report["cases"][f"{profile}/{variant}"] = {
                "engine": engine,
                "baseline_engine": baseline_engine,
                "source_sha256": {
                    n: hashlib.sha256(b).hexdigest() for n, b in files.items()
                },
                "bridge": data,
                "electrical_residuals": checks,
                "baseline_equivalence": equality,
                "independent_linear_solve": independent,
                "passed": True,
            }
            print(
                profile,
                variant,
                "PASS",
                len(checks),
                "electrical comparisons",
                flush=True,
            )
    if (
        max(
            abs(baseline_snapshots["pnnl"][k] - baseline_snapshots["arras"][k])
            for k in baseline_snapshots["pnnl"]
        )
        < 1e-3
    ):
        raise ValueError("conditional did not distinguish the native profiles")
    report["native_profile_difference_verified"] = True
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

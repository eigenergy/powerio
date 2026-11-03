"""Compare native project loading with PowerIO's bounded GLM preprocessing.

This validates preprocessing and source syntax only, not network conversion or
canonical writing. Every case must also match the independent electrical oracle.
"""

import argparse
import hashlib
import json
from pathlib import Path

from oracle import analytic, cases, command, compare, glm, read_glm, run_glm


def files() -> dict[str, str]:
    model = glm(*cases()["single_phase"])
    model = model.replace("nominal_voltage 240;", "nominal_voltage ${VN};")
    configuration = "object line_configuration { name cfg;\nz11 1+0.5j;\n}"
    if configuration not in model:
        raise ValueError("oracle line configuration template changed")
    model = model.replace(configuration, '#include "parts/configuration.glm"')
    model = model.replace(
        "constant_impedance_A 20+5j;", "constant_impedance_A ${R}+${X}j;"
    )
    model = (
        """#define VN=240
#define SELECT=100
#if SELECT<20
#define R=20
#else
#define R=40
#endif
#ifdef NEVER_DEFINED_IN_THIS_CASE
deliberately invalid { unterminated syntax
#endif
#include "parts/settings.glm"
"""
        + model
    )
    return {
        "model.glm": model,
        "parts/settings.glm": '#include "reactance.glm"\n#define X=${REACTANCE}\n',
        "reactance.glm": "#define REACTANCE=5\n",
        "parts/configuration.glm": configuration + "\n",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--preprocessor", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "profiles": {},
        "cases": {},
    }
    for profile in ("pnnl", "arras"):
        project = output / f"{profile}-project"
        project.mkdir()
        inputs = files()
        source, impedance, loads = cases()["single_phase"]
        if profile == "arras":
            # Arras defaults to literal lhs: "SELECT" < "20" is false.
            loads[0] = 40 + 5j
        reference = analytic(source, impedance, loads)
        for name, text in inputs.items():
            path = project / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        expanded = command(
            [str(args.preprocessor.resolve()), str(project / "model.glm"), profile],
            output,
        )
        (output / f"{profile}-expanded.glm").write_text(expanded)
        report["profiles"][profile] = {
            "expected_load_ohm": [float(loads[0].real), float(loads[0].imag)],
            "input_sha256": {
                k: hashlib.sha256(v.encode()).hexdigest() for k, v in inputs.items()
            },
            "expanded_sha256": hashlib.sha256(expanded.encode()).hexdigest(),
        }
        snapshots = {}
        for variant in ("original", "expanded"):
            directory = output / f"{profile}-{variant}"
            directory.mkdir()
            content = inputs if variant == "original" else {"model.glm": expanded}
            for name, text in content.items():
                path = directory / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
            version = run_glm(profile, directory, args.pnnl_root.resolve())
            snapshots[variant] = read_glm(directory, 1)
            agreement = compare(reference, snapshots[variant])
            if not agreement["passed"]:
                raise ValueError(
                    f"{profile}-{variant} failed analytic reference: {agreement}"
                )
            report["cases"][f"{profile}-{variant}"] = {
                "engine": version,
                "comparison": agreement,
            }
        agreement = compare(snapshots["original"], snapshots["expanded"])
        if not agreement["passed"]:
            raise ValueError(
                f"{profile} preprocessing changed electrical results: {agreement}"
            )
        report["cases"][f"{profile}-native-vs-expanded"] = agreement
        print(profile, "PASS native, expanded, analytic comparisons")
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

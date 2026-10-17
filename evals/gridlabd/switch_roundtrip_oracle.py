"""Qualify fresh, IR-restored and edited fixed-contact GLM against analytic physics.

Run switch_state_oracle.py first. Its hashed native inputs and successful reports
provide the original fixed-state reference. Canonical decks come exclusively from
typed PowerIO values via glm_roundtrip. The independent reference changes source
A magnitude, line impedances and load admittances by the bridge's stated edits.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

from oracle import cases, command, compare, run_glm
from switch_state_oracle import reference, snapshot, variants


def impedance_run(profile, directory, data, expected, ideal, pnnl_root):
    from switch_oracle import snapshot as single_snapshot

    directory.mkdir()
    (directory / "model.glm").write_text(data)
    result = {
        "passed": False,
        "input_sha256": hashlib.sha256(data.encode()).hexdigest(),
    }
    try:
        result["version"] = run_glm(profile, directory, pnnl_root)
        actual = single_snapshot(directory)
        comparison = compare(expected, actual)
        negative = compare(ideal, actual)
        result.update(
            passed=comparison["passed"] and not negative["passed"],
            comparison=comparison,
            ideal_negative_control=negative,
        )
    except Exception as error:
        # Exit zero is not a solution: both pinned NR engines can emit NaNs.
        # Retain the failed case and continue collecting the other witnesses.
        result["error"] = str(error)
    return result


def impedance_failure_probes(args, profile, target, data, z, expected, ideal):
    from oracle import literal

    native, count = re.subn(
        r"object overhead_line \{[^}]*\}",
        "object switch { name line; phases AN; from source; to sink; "
        f"switch_impedance {literal(z)}; }}",
        data,
    )
    if count != 1 or data.count("solver_method NR;") != 1:
        raise ValueError("failure probes require the single-link NR fixture")
    return {
        name: impedance_run(
            profile, target / name, deck, expected, ideal, args.pnnl_root
        )
        for name, deck in (
            ("native_switch_nr", native),
            ("canonical_fbs", data.replace("solver_method NR;", "solver_method FBS;")),
        )
    }


def check_impedances(args, report):
    from roundtrip_oracle import instrumentation as single_instrumentation
    from switch_oracle import (
        reference as single_reference,
    )
    from switch_oracle import (
        variants as impedance_variants,
    )

    baseline = json.loads((args.impedance_calibration / "report.json").read_text())[
        "cases"
    ]
    for profile in ("pnnl", "arras"):
        for name, _, _, impedance in impedance_variants():
            if impedance == 0:
                # This harness qualifies finite contact impedances. Exact FBS
                # output is covered separately by ideal_switch_oracle.py.
                continue
            key = f"{profile}/impedance_{name}"
            source = args.impedance_calibration / profile / name / "model.glm"
            witness = baseline[f"{profile}/{name}"]
            if (
                not witness["passed"]
                or hashlib.sha256(source.read_bytes()).hexdigest()
                != witness["input_sha256"]
            ):
                raise ValueError(f"unverified impedance baseline {key}")
            directory = args.output / profile / ("impedance_" + name)
            directory.mkdir(parents=True)
            command(
                [
                    str(args.bridge.resolve()),
                    str(source.resolve()),
                    str(directory.resolve()),
                    profile,
                ],
                directory,
            )
            if (directory / "fresh.glm").read_bytes() != (
                directory / "restored.glm"
            ).read_bytes():
                raise ValueError("IR changed canonical contact impedance")
            for variant in ("fresh", "restored", "edited"):
                target = directory / variant
                data = (
                    directory / f"{variant}.glm"
                ).read_text() + single_instrumentation("A")
                voltages, _, loads = cases()["single_phase"]
                z = impedance
                if variant == "edited":
                    voltages[0] *= 1.03
                    loads /= 1.25
                    z *= 1.1
                expected = single_reference(voltages, loads, z)
                ideal = single_reference(voltages, loads, 0j)
                result = impedance_run(
                    profile, target, data, expected, ideal, args.pnnl_root
                )
                if not result["passed"]:
                    result["diagnostic_runs"] = impedance_failure_probes(
                        args, profile, target, data, z, expected, ideal
                    )
                    # An alternate solver does not qualify the emitted NR deck.
                    # Keep the primary result failed even if FBS matches exactly.
                report["cases"][f"{key}/{variant}"] = result
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(
                    f"{key}/{variant}: {'passed' if result['passed'] else 'FAILED'}",
                    flush=True,
                )


def instrumentation(net):
    text = """
#set complex_format=%.15g%+.15g%c
#set complex_output_format=DEFAULT
module tape;
clock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:02'; }
"""
    outputs = ["source.csv", "sink.csv"]
    for bus in ("source", "sink"):
        text += f'object recorder {{ parent {bus}; property voltage_A,voltage_B,voltage_C; interval 1; file "{bus}.csv"; }}\n'
    properties = (
        ",".join(
            f"{q}_{p}"
            for p in "ABC"
            for q in ("current_in", "current_out", "power_in", "power_out")
        )
        + ",power_losses"
    )
    contacts = []
    branches = net["lines"] + net["switches"]
    for branch in branches:
        name = branch["name"]
        if name in ("bypass", "bypass_end"):
            filename = name + ".csv"
        elif branch["bus_from"] == "source" and branch["bus_to"] == "sink":
            filename = f"contact-{len(contacts)}.csv"
            contacts.append(filename)
        else:
            raise ValueError(f"unexpected branch identity {name}")
        outputs.append(filename)
        text += f'object recorder {{ parent {name}; property {properties}; interval 1; file "{filename}"; }}\n'
    if not contacts or not {"bypass.csv", "bypass_end.csv"}.issubset(outputs):
        raise ValueError("missing contact or bypass identity")
    return text, tuple(outputs), contacts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--calibration", type=Path, required=True)
    parser.add_argument("--impedance-calibration", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    args = parser.parse_args()
    baseline = json.loads((args.calibration / "report.json").read_text())["cases"]
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    for profile in ("pnnl", "arras"):
        for name, _, _, closed in variants():
            key = f"{profile}/{name}"
            source = args.calibration / profile / name / "model.glm"
            witness = baseline[key]
            if (
                not witness["passed"]
                or hashlib.sha256(source.read_bytes()).hexdigest()
                != witness["input_sha256"]
            ):
                raise ValueError(f"unverified native baseline {key}")
            directory = args.output / profile / name
            directory.mkdir(parents=True)
            decoded = json.loads(
                command(
                    [
                        str(args.bridge.resolve()),
                        str(source.resolve()),
                        str(directory.resolve()),
                        profile,
                    ],
                    directory,
                )
            )
            if (directory / "fresh.glm").read_bytes() != (
                directory / "restored.glm"
            ).read_bytes():
                raise ValueError("IR changed canonical GLM")
            for variant in ("fresh", "restored", "edited"):
                net = decoded[variant]["network"]
                extra, outputs, contacts = instrumentation(net)
                target = directory / variant
                target.mkdir()
                data = (directory / f"{variant}.glm").read_text() + extra
                (target / "model.glm").write_text(data)
                version = run_glm(profile, target, args.pnnl_root, outputs=outputs)
                voltages, bypass_z, loads = cases()["coupled_unbalanced"]
                switch_z = 1 + 0.5j
                if variant == "edited":
                    voltages[0] *= 1.03
                    bypass_z *= 1.1
                    switch_z *= 1.1
                    loads /= 1.25
                expected = reference(voltages, bypass_z, loads, closed, switch_z)
                actual = snapshot(target, contacts)
                comparison = compare(expected, actual)
                opposite = compare(
                    reference(
                        voltages,
                        bypass_z,
                        loads,
                        tuple(not x for x in closed),
                        switch_z,
                    ),
                    actual,
                )
                result = {
                    "passed": comparison["passed"] and not opposite["passed"],
                    "version": version,
                    "input_sha256": hashlib.sha256(data.encode()).hexdigest(),
                    "comparison": comparison,
                    "opposite_state_negative_control": opposite,
                    "contact_measurements": contacts,
                    "diagnostics": decoded[variant]["diagnostics"],
                }
                report["cases"][f"{key}/{variant}"] = result
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(
                    f"{key}/{variant}: {'passed' if result['passed'] else 'FAILED'}",
                    flush=True,
                )
    if args.impedance_calibration:
        check_impedances(args, report)
    return 0 if all(case["passed"] for case in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())

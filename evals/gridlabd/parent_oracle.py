"""Calibrate electrical parent chains in the pinned GridLAB-D profiles.

Keep a coupled feeder and a direct sibling load while moving an extra load
through node/meter/load ancestors. Compare native voltages, port currents,
powers and losses with independent included- and omitted-load references.
Declaration order and phase subsets are explicit case dimensions.
"""

import argparse
import hashlib
import json
from pathlib import Path

from oracle import compare, literal, read_glm, run_glm
from pair_load_oracle import deck as pair_deck
from pair_load_oracle import reference

EXTRA = [("impedance", "AB", 80 + 20j)]
SIBLING = [("impedance", "CN", 100 + 10j)]


def variants():
    yield "direct", 1, "node", False, False
    for parent_class in ("node", "meter", "load"):
        for depth in (2, 3):
            yield f"{parent_class}_{depth}", depth, parent_class, False, False
        yield f"{parent_class}_2_leaf_first", 2, parent_class, True, False
    yield "meter_2_subset", 2, "meter", False, True
    yield "meter_3_leaf_first_subset", 3, "meter", True, True
    yield "load_3_piz", 3, "load", False, False
    yield "load_3_piz_leaf_first", 3, "load", True, False


def full_terms(spec):
    return (
        SIBLING
        + EXTRA
        + (
            [("power", "AN", 150 + 30j), ("current", "BN", -0.3 - 0.6j)]
            if "piz" in spec[0]
            else []
        )
    )


def omitted_terms(spec):
    # The direct child retains its own power demand when deeper terms vanish.
    return SIBLING + ([("power", "AN", 150 + 30j)] if "piz" in spec[0] else [])


def deck(spec, solver):
    _, depth, parent_class, reverse, subset = spec
    text = pair_deck([], "NR").replace("solver_method NR;", f"solver_method {solver};")
    # pair_deck's FBS option adds an ideal link, which is unrelated to this
    # parent calibration. Direct replacement retains the same single line.
    objects = []
    parent = "sink"
    for i in range(1, depth):
        name = f"tier{i}"
        phases = "ABN" if subset else "ABCN"
        own = ""
        if "piz" in spec[0]:
            own = (
                "constant_power_AN 150+30j; "
                if i == 1
                else "constant_current_BN -0.3-0.6j; "
            )
        objects.append(
            f"object {parent_class} {{ name {name}; parent {parent}; phases {phases}; nominal_voltage 240; {own}}}"
        )
        parent = name
    phases = "ABN" if subset else "ABCN"
    objects.append(
        f"object load {{ name nested; parent {parent}; phases {phases}; nominal_voltage 240; constant_impedance_AB {literal(EXTRA[0][2])}; }}"
    )
    if reverse:
        objects.reverse()
    objects.append(
        f"object load {{ name sibling; parent sink; phases ABCN; nominal_voltage 240; constant_impedance_CN {literal(SIBLING[0][2])}; }}"
    )
    return text + "\n" + "\n".join(objects) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--case", action="append", choices=[v[0] for v in variants()])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    for profile in ("pnnl", "arras"):
        for solver in ("NR", "FBS"):
            for spec in variants():
                if args.case and spec[0] not in args.case:
                    continue
                full, _ = reference(full_terms(spec))
                omitted, _ = reference(omitted_terms(spec))
                key = f"{profile}/{solver}/{spec[0]}"
                directory = args.output / key
                directory.mkdir(parents=True)
                text = deck(spec, solver)
                (directory / "model.glm").write_text(text)
                item = {"input_sha256": hashlib.sha256(text.encode()).hexdigest()}
                try:
                    item["version"] = run_glm(profile, directory, args.pnnl_root)
                    actual = read_glm(directory, 3)
                    item["included_reference"] = compare(full, actual)
                    item["omitted_reference"] = compare(omitted, actual)
                    item["classification"] = (
                        "included"
                        if item["included_reference"]["passed"]
                        else "omitted"
                        if item["omitted_reference"]["passed"]
                        else "unexplained"
                    )
                except Exception as error:
                    item["classification"] = "engine_failure"
                    item["error"] = str(error)
                report["cases"][key] = item
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(key, item["classification"], flush=True)
    return (
        0
        if all(
            v["classification"] in ("included", "omitted")
            for v in report["cases"].values()
        )
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())

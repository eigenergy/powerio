"""Qualify feeder physical-port aggregation on native current-form microcases."""

import argparse
import copy
import json
import tempfile
from pathlib import Path

from current_form_write_oracle import expected
from feeder_balance import current_balance
from ieee9500_import_oracle import imported_snapshot
from oracle import add, command, compare


def network(value):
    value = value.get("value", value)
    return value.get("network", value)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cases", type=Path, action="append", required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--bridge", type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"cases": {}}
    for directory in args.cases:
        specification = json.loads((directory / "report.json").read_text())
        phase = specification["phase"]
        for label, qualification in specification["cases"].items():
            if not qualification["passed"]:
                continue
            law, profile = label.split("/")
            folder = directory / law / profile
            with tempfile.TemporaryDirectory(dir=args.output) as temp:
                stages = json.loads(
                    command(
                        [
                            str(args.bridge.resolve()),
                            str((folder / "input.glm").resolve()),
                            temp,
                            profile,
                        ],
                        args.output,
                    )
                )
            for stage, value in stages.items():
                net = network(value["network"])
                line = net["lines"][0]
                code = next(
                    c for c in net["linecodes"] if c["name"] == line["linecode"]
                )
                z = (
                    complex(code["r_series"][0][0], code["x_series"][0][0])
                    * line["length"]
                )
                source = net["sources"][0]
                source = source.get("value", source)
                vs = source["v_magnitude"][0]
                load = net["loads"][0]
                model = load["voltage_model"]
                vnom = model["v_nom"][0] if model["v_nom"] else vs
                wanted = expected(
                    z, vs, complex(load["p_nom"][0], load["q_nom"][0]), law, vnom
                )
                snapshot = {}
                for channel in "ABC":
                    add(
                        snapshot,
                        f"bus.source.{channel}",
                        "V",
                        vs if channel == phase else 0j,
                    )
                    add(
                        snapshot,
                        f"bus.sink.{channel}",
                        "V",
                        complex(*wanted["voltage"]["value"])
                        if channel == phase
                        else 0j,
                    )
                    for side in ["in", "out"]:
                        endpoint = "from" if side == "in" else "to"
                        for quantity, unit in [("current", "A"), ("power", "VA")]:
                            add(
                                snapshot,
                                f"link.line.{side}.{channel}.{quantity}",
                                unit,
                                complex(*wanted[f"{endpoint}.{quantity}"]["value"])
                                if channel == phase
                                else 0j,
                            )
                add(snapshot, "link.line.loss", "VA", complex(*wanted["loss"]["value"]))
                objects = {
                    "source": {"class": "node", "phases": phase + "N"},
                    "sink": {"class": "load", "phases": phase + "N"},
                    "line": {
                        "class": "overhead_line",
                        "from": "source",
                        "to": "sink",
                        "phases": phase + "N",
                    },
                }
                paths = [
                    ("canonical", folder / f"emitted/{stage}.dss/case.dss", net),
                    (
                        "dss_reimported",
                        folder / stage / "dss_reimported/dss/case.dss",
                        net,
                    ),
                ]
                for variant, path, state in paths:
                    actual, solve = imported_snapshot(path, state, objects)
                    check = compare(snapshot, actual)
                    balance = current_balance(actual, state, objects, ["overhead_line"])
                    changed = copy.deepcopy(state)
                    for load in changed["loads"]:
                        load["p_nom"] = [p * 1.1 for p in load["p_nom"]]
                        load["q_nom"] = [q * 1.1 for q in load["q_nom"]]
                    negative = current_balance(
                        actual, changed, objects, ["overhead_line"]
                    )
                    check["current_balance"] = balance
                    check["changed_load_negative"] = negative
                    check["passed"] = (
                        check["passed"] and balance["passed"] and not negative["passed"]
                    )
                    if solve["current_form_lines"] != ["line"]:
                        raise ValueError(
                            "microcase did not exercise current-form aggregation"
                        )
                    report["cases"][f"{directory.name}/{label}/{stage}/{variant}"] = (
                        check
                    )
    report["passed"] = bool(report["cases"]) and all(
        c["passed"] for c in report["cases"].values()
    )
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        report["passed"],
        len(report["cases"]),
        [c["failures"][:2] for c in report["cases"].values() if not c["passed"]][:5],
    )
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

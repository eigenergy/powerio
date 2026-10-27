"""Fixed generator/IBR signed-load output, edited and repeated native circuits."""

import argparse
import hashlib
import json
import re
from pathlib import Path

from diesel_import_oracle import CASES as DIESEL_CASES
from diesel_import_oracle import SOURCE, observations, powers, recorded
from diesel_import_oracle import deck as diesel_deck
from glm_service_write_oracle import instrument as replay_instruments
from oracle import LOCK, add, command, compare, run_glm
from reactor_source_oracle import native as ordinary_native
from solar_import_oracle import CASES as SOLAR_CASES
from solar_import_oracle import deck as solar_deck
from solar_import_oracle import expected as solar_power
from solar_import_oracle import service_native
from split_phase_import_oracle import read_dss as service_dss


def network_data(value):
    while "type" in value:
        value = value["value"]
        if "network" in value:
            value = value["network"]
    return value


def instrument(text, original, names):
    # The original machine/inverter recorder cannot address a load equivalent.
    source = re.sub(
        r'object recorder\s*\{[^{}]*file (?:"?dg.csv"?|"?inv.csv"?);[^{}]*\}',
        "",
        original,
    )
    text = replay_instruments(text, source)
    text = re.sub(
        r"(parent line;\s*property )([^;]+);",
        r"\1\2,current_out_A,current_out_B,current_out_C;",
        text,
    )
    return text


def diesel_analytic(case, edited):
    z = DIESEL_CASES[case][3] * (1.1 if edited else 1)
    s = [v * (1.25 if edited else 1) for v in powers(case)]
    source = list(SOURCE)
    if edited:
        source[0] *= 1.03
    v = source[:]
    for _ in range(1000):
        new = [
            a + z * (p / b).conjugate() for a, p, b in zip(source, s, v, strict=True)
        ]
        if max(abs(a - b) for a, b in zip(new, v, strict=True)) < 1e-12:
            return observations(
                new, [-(p / a).conjugate() for p, a in zip(s, new, strict=True)], s
            )
        v = new
    raise ValueError("analytic generator circuit did not converge")


def dss_observe(path, kind, case, phase, names):
    import opendssdirect as dss

    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("OpenDSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    e.Text.Command("set tolerance=1e-12 maxiterations=1000 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged() or e.Solution.ControlMode() != -1:
        raise ValueError("DSS solve failed")

    def unpack(v):
        return [complex(*v[i : i + 2]) for i in range(0, len(v), 2)]

    injection = {}
    elements = {name.lower() for name in e.Circuit.AllElementNames()}
    for name in names:
        selected = [
            value
            for value in elements
            if value in ("generator." + name.lower(), "load." + name.lower())
            or value.startswith("load." + name.lower() + "__glm_power_")
        ]
        if not selected:
            raise ValueError(f"missing injection {name}")
        power = 0j
        for element in selected:
            e.Circuit.SetActiveElement(element)
            power -= 1000 * sum(unpack(e.CktElement.Powers()))
        injection[name] = power

    def volts(bus):
        e.Circuit.SetActiveBus(bus)
        values = dict(zip(e.Bus.Nodes(), unpack(e.Bus.Voltages()), strict=True))
        return {p: values[i + 1] for i, p in enumerate("ABC")}

    if kind == "diesel":
        sink = volts("sink")
        e.Circuit.SetActiveElement("line.line")
        current = unpack(e.CktElement.Currents())[:3]
        return observations(
            [sink[p] for p in "ABC"], current, [injection[n] for n in names]
        )
    if SOLAR_CASES[case][0]:
        result = service_dss(path, phase)
    else:
        from reactor_source_oracle import snapshot

        source, sink = volts("source"), volts("sink")
        e.Circuit.SetActiveElement("line.reactor")
        current = unpack(e.CktElement.Currents())
        result = snapshot(
            "ABC",
            source,
            sink,
            dict(zip("ABC", current[:3], strict=True)),
            dict(zip("ABC", current[3:], strict=True)),
        )
    add(result, "inverter.power", "VA", sum(injection.values()))
    return result


def background_power(network, result, triplex):
    if triplex:
        v = {
            "1": complex(*result["sink.L1.voltage"]["value"]),
            "2": complex(*result["sink.L2.voltage"]["value"]),
            "n": 0j,
        }
    else:
        v = {
            str(i + 1): complex(*result[f"{p}.to.voltage"]["value"])
            for i, p in enumerate("ABC")
        }
        v["n"] = 0j
    power = 0j
    for load in network["loads"]:
        if load["bus"] != "sink":
            continue
        terminals = load["terminal_map"]
        connections = (
            [(terminals[0], terminals[1])]
            if len(terminals) == 2
            else [(p, terminals[-1]) for p in terminals[:-1]]
        )
        model = load["voltage_model"]
        for i, (a, b) in enumerate(connections):
            coefficient = complex(load["p_nom"][i], load["q_nom"][i])
            if model["model"] == "constant_power":
                power += coefficient
            elif model["model"] in ("constant_current", "constant_impedance"):
                power += coefficient * (abs(v[a] - v[b]) / model["v_nom"][i]) ** (
                    1 if model["model"] == "constant_current" else 2
                )
            else:
                raise ValueError("background load law is not qualified")
    return power


def native_observe(folder, kind, case, phase, network):
    if kind == "diesel":
        v = recorded(folder / "sink.csv")
        i = recorded(folder / "line.csv")
        values = [-v[f"voltage_{p}"] * i[f"current_out_{p}"].conjugate() for p in "ABC"]
        return observations(
            [v[f"voltage_{p}"] for p in "ABC"],
            [i[f"current_in_{p}"] for p in "ABC"],
            values,
        )
    triplex = SOLAR_CASES[case][0]
    result = (
        service_native(folder, phase) if triplex else ordinary_native(folder, "ABC")
    )
    keys = (
        [f"transformer.to.{p}.power" for p in ("L1", "L2", "N")]
        if triplex
        else [f"{p}.to.power" for p in "ABC"]
    )
    into = sum(complex(*result[k]["value"]) for k in keys)
    add(
        result,
        "inverter.power",
        "VA",
        background_power(network, result, triplex) + into,
    )
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(
        engines=LOCK,
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
    )
    for kind, cases in (("diesel", DIESEL_CASES), ("solar", SOLAR_CASES)):
        for case in cases:
            label = f"{kind}-{case}"
            if args.case and label not in args.case:
                continue
            for solver in ("NR", "FBS") if kind == "diesel" else ("original",):
                phase, text = (
                    ("ABC", diesel_deck(case, solver))
                    if kind == "diesel"
                    else solar_deck(case)
                )
                profiles = ("pnnl",) if kind == "diesel" else ("pnnl", "arras")
                for profile in profiles:
                    folder = args.output / label / solver / profile
                    folder.mkdir(parents=True)
                    source = folder / "input.glm"
                    source.write_text(text)
                    row = dict(passed=False, comparisons={})
                    try:
                        emitted = folder / "emitted"
                        data = json.loads(
                            command(
                                [
                                    str(args.bridge.resolve()),
                                    str(source.resolve()),
                                    str(emitted.resolve()),
                                    profile,
                                ],
                                folder,
                            )
                        )
                        (folder / "bridge.json").write_text(
                            json.dumps(data, indent=2) + "\n"
                        )
                        raw_network = data["fresh"]["network"]
                        network = network_data(raw_network)
                        names = [
                            x["name"]
                            for x in network[
                                "generators" if kind == "diesel" else "ibrs"
                            ]
                        ]
                        for stage in (
                            "fresh",
                            "restored",
                            "edited",
                            "rebased",
                            "grounded_reference",
                        ):
                            wanted = (
                                diesel_analytic(case, stage == "edited")
                                if kind == "diesel"
                                else dss_observe(
                                    emitted / f"{stage}.dss/case.dss",
                                    kind,
                                    case,
                                    phase,
                                    names,
                                )
                            )
                            if kind == "solar":
                                power = {}
                                add(
                                    power,
                                    "inverter.power",
                                    "VA",
                                    solar_power(case)
                                    * (1.25 if stage == "edited" else 1),
                                )
                                row["comparisons"][stage + "/independent-power"] = (
                                    compare(
                                        power,
                                        {"inverter.power": wanted["inverter.power"]},
                                    )
                                )
                            repeat = folder / ("reimported-" + stage)
                            command(
                                [
                                    str(args.bridge.resolve()),
                                    str((emitted / f"{stage}.glm").resolve()),
                                    str(repeat.resolve()),
                                    profile,
                                ],
                                folder,
                            )
                            for variant, path in (
                                ("canonical", emitted / f"{stage}.glm"),
                                ("reimported", repeat / "fresh.glm"),
                            ):
                                run = folder / stage / variant
                                run.mkdir(parents=True)
                                (run / "model.glm").write_text(
                                    instrument(path.read_text(), text, names)
                                )
                                files = (
                                    ("sink.csv", "line.csv")
                                    if kind == "diesel"
                                    else (
                                        ("source.csv", "sink.csv", "line.csv")
                                        if SOLAR_CASES[case][0]
                                        else ("source.csv", "sink.csv", "reactor.csv")
                                    )
                                )
                                run_glm(
                                    profile,
                                    run,
                                    args.pnnl_root,
                                    outputs=files,
                                )
                                actual = native_observe(
                                    run,
                                    kind,
                                    case,
                                    phase,
                                    network_data(data[stage]["network"]),
                                )
                                row["comparisons"][stage + "/" + variant] = compare(
                                    wanted, actual
                                )
                                (run / "measurements.json").write_text(
                                    json.dumps(
                                        dict(expected=wanted, actual=actual), indent=2
                                    )
                                    + "\n"
                                )
                                if stage == "edited" and variant == "canonical":
                                    edited_actual = actual
                            for variant, path in (
                                ("dss", emitted / f"{stage}.dss/case.dss"),
                                ("reimported-dss", repeat / "fresh.dss/case.dss"),
                            ):
                                if kind == "solar" and variant == "dss":
                                    continue
                                row["comparisons"][stage + "/" + variant] = compare(
                                    wanted, dss_observe(path, kind, case, phase, names)
                                )
                        baseline = (
                            diesel_analytic(case, False)
                            if kind == "diesel"
                            else dss_observe(
                                emitted / "fresh.dss/case.dss", kind, case, phase, names
                            )
                        )
                        row["unchanged_negative"] = compare(baseline, edited_actual)
                        row["passed"] = (
                            all(x["passed"] for x in row["comparisons"].values())
                            and not row["unchanged_negative"]["passed"]
                        )
                    except Exception as error:
                        row["error"] = str(error)
                    report["cases"][f"{label}/{solver}/{profile}"] = row
                    (args.output / "report.json").write_text(
                        json.dumps(report, indent=2) + "\n"
                    )
                    print(
                        label,
                        solver,
                        profile,
                        row["passed"],
                        row.get("error", ""),
                        {
                            k: v["failures"][:2]
                            for k, v in row["comparisons"].items()
                            if not v["passed"]
                        },
                        flush=True,
                    )
    report["passed"] = bool(report["cases"]) and all(
        x["passed"] for x in report["cases"].values()
    )
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

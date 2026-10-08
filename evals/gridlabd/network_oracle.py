"""Qualify GLM -> MulticonductorNetwork -> IR against two pinned engines.

At each engine's solved voltage, independently evaluate the imported line pi
model and each load's voltage law. Compare branch currents, powers, losses and
KCL. The original constant-Z circuits also have an independent linear solve.
This is electrical reader/IR evidence, not canonical GLM writer qualification.
"""

import argparse
import copy
import hashlib
import json
from pathlib import Path

import numpy as np

from oracle import analytic, cases, command, compare, glm, read_glm, recorded, run_glm


def decks():
    single = glm(*cases()["single_phase"])
    three = glm(*cases()["coupled_unbalanced"])
    yield "single_z", single, "A", cases()["single_phase"]
    yield "three_z", three, "ABC", cases()["coupled_unbalanced"]
    yield (
        "phase_c",
        single.replace("AN", "CN").replace("_A", "_C").replace("z11", "z33"),
        "C",
        None,
    )
    currents = three.replace(
        "maximum_voltage_error 1e-10;",
        """maximum_voltage_error 1e-10;
        constant_current_A 1-.2j; constant_current_B -.5-.7j;
        constant_current_C -.3+.8j;
        constant_power_A 200+40j; constant_power_B 300+50j;
        constant_power_C 100+30j;""",
    )
    yield "mixed_piz", currents, "ABC", None
    shifted = currents.replace("voltage_A 240+0j;", "voltage_A 230+30j;")
    if shifted == currents:
        raise ValueError("source template no longer matches")
    yield "shifted_source_piz", shifted, "ABC", None
    yield (
        "line_capacitance",
        single.replace(
            "solver_method NR;", "solver_method NR; line_capacitance TRUE;"
        ).replace("z11 1+0.5j;", "z11 1+0.5j; c11 100000;"),
        "A",
        None,
    )
    parented = single.replace(
        "object load { name sink;", "object node { name sink;"
    ).replace("constant_impedance_A 20+5j;", "")
    parented += "object load { name demand; parent sink; phases AN; constant_impedance_A 20+5j; }\n"
    yield "parented_load", parented, "A", None
    yield (
        "parented_load_fbs",
        parented.replace("solver_method NR", "solver_method FBS"),
        "A",
        None,
    )


def residuals(net, directory, phases):
    voltages = {}
    for name in ("source", "sink"):
        measurements = recorded(directory / f"{name}.csv")
        voltages[name] = {
            str("ABC".index(p) + 1): measurements[f"voltage_{p}"] for p in phases
        }
        voltages[name]["n"] = 0j
    line = net["lines"][0]
    code = next(c for c in net["linecodes"] if c["name"] == line["linecode"])
    z = (np.array(code["r_series"]) + 1j * np.array(code["x_series"])) * line["length"]
    yf = (np.array(code["g_from"]) + 1j * np.array(code["b_from"])) * line["length"]
    yt = (np.array(code["g_to"]) + 1j * np.array(code["b_to"])) * line["length"]
    vf = np.array([voltages[line["bus_from"]][t] for t in line["terminal_map_from"]])
    vt = np.array([voltages[line["bus_to"]][t] for t in line["terminal_map_to"]])
    series_current = np.linalg.solve(z, vf - vt)
    from_current = series_current + yf @ vf
    to_current = -series_current + yt @ vt
    measured = recorded(directory / "line.csv")
    checks = {}

    def check(name, actual, expected, atol, rtol=2e-8):
        error = abs(actual - expected)
        bound = atol + rtol * abs(expected)
        checks[name] = {"absolute_error": float(error), "bound": float(bound)}
        if not error <= bound:
            raise ValueError(f"{name}: {actual} != {expected}, error {error} > {bound}")

    for i, phase in enumerate(phases):
        for side, current, voltage, native, sign in (
            ("from", from_current[i], vf[i], "in", 1),
            ("to", to_current[i], vt[i], "out", -1),
        ):
            check(
                f"line.{side}.{phase}.current",
                current,
                sign * measured[f"current_{native}_{phase}"],
                1e-8,
            )
            check(
                f"line.{side}.{phase}.power",
                voltage * current.conjugate(),
                sign * measured[f"power_{native}_{phase}"],
                1e-6,
            )
    loss = sum(vf * from_current.conjugate() + vt * to_current.conjugate())
    check("line.loss", loss, measured["power_losses"], 1e-6)
    load_currents = dict.fromkeys(line["terminal_map_to"], 0j)
    for load in net["loads"]:
        if load["bus"] != "sink" or load["configuration"] != "wye":
            raise ValueError("unexpected oracle load topology")
        model = load["voltage_model"]
        exponent = {
            "constant_power": 0,
            "constant_current": 1,
            "constant_impedance": 2,
        }[model["model"]]
        for i, terminal in enumerate(load["terminal_map"][:-1]):
            voltage = voltages["sink"][terminal]
            nominal_power = complex(load["p_nom"][i], load["q_nom"][i])
            power = nominal_power * (abs(voltage) / model["v_nom"][i]) ** exponent
            load_currents[terminal] += (power / voltage).conjugate()
    for i, terminal in enumerate(line["terminal_map_to"]):
        check(f"sink.{terminal}.kcl", to_current[i] + load_currents[terminal], 0, 2e-7)
    for source in net["sources"]:
        for terminal, magnitude, angle in zip(
            source["terminal_map"],
            source["v_magnitude"],
            source["v_angle"],
            strict=True,
        ):
            check(
                f"source.{terminal}.voltage",
                magnitude * np.exp(1j * angle),
                voltages["source"][terminal],
                1e-8,
            )
    return checks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--reader", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    for profile in ("pnnl", "arras"):
        for name, text, phases, linear_case in decks():
            directory = args.output.resolve() / profile / name
            directory.mkdir(parents=True)
            source = directory / "model.glm"
            source.write_text(text)
            parsed = json.loads(
                command(
                    [
                        str(args.reader.resolve()),
                        str(source),
                        str(directory / "network.pio.json"),
                        profile,
                    ],
                    directory,
                )
            )
            version = run_glm(
                profile,
                directory,
                args.pnnl_root.resolve(),
                ("source.csv", "sink.csv", "line.csv", "impedance.xml"),
            )
            checks = residuals(parsed["network"], directory, phases)
            negative_controls = []
            mutations = ["line_resistance", "load_power", "source_angle"]
            if name == "line_capacitance":
                mutations.append("missing_line_capacitance")
            for mutation in mutations:
                wrong = copy.deepcopy(parsed["network"])
                if mutation == "line_resistance":
                    wrong["linecodes"][0]["r_series"][0][0] *= 2
                elif mutation == "load_power":
                    wrong["loads"][0]["p_nom"][0] *= 1.1
                elif mutation == "source_angle":
                    wrong["sources"][0]["v_angle"][0] += 0.01
                else:
                    wrong["linecodes"][0]["b_from"][0][0] = 0
                    wrong["linecodes"][0]["b_to"][0][0] = 0
                try:
                    residuals(wrong, directory, phases)
                except ValueError:
                    negative_controls.append(mutation)
                else:
                    raise ValueError(f"oracle missed deliberate {mutation} error")
            independent = None
            if linear_case is not None:
                independent = compare(
                    analytic(*linear_case), read_glm(directory, len(phases))
                )
                if not independent["passed"]:
                    raise ValueError("independent constant-Z solution disagrees")
            report["cases"][f"{profile}/{name}"] = {
                "engine": version,
                "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
                "reader": parsed,
                "detected_negative_controls": negative_controls,
                "electrical_residuals": checks,
                "independent_linear_solve": independent,
                "passed": True,
            }
            print(
                profile, name, "PASS", len(checks), "electrical comparisons", flush=True
            )
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

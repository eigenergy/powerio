"""Exact scalar current-form lines through typed edits, IR and repeated output."""

import argparse
import hashlib
import json
import re
from pathlib import Path

import opendssdirect as dss
from oracle import (
    FEEDER_TOLERANCES,
    LOCK,
    add,
    command,
    compare,
    parse_complex,
    recorded,
    run_glm,
)


def deck(z, voltage, law, phase):
    suffix, value = {
        "Z": ("impedance", -120j),
        "P": ("power", 1000 + 200j),
        "I": ("current", 1 - 0.2j),
    }[law]

    def literal(v):
        return f"{v.real:.17g}{v.imag:+.17g}j"

    text = f"""module powerflow {{ solver_method NR; }}
object node {{ name source; phases AN; bustype SWING; nominal_voltage {voltage}; voltage_A {voltage}+0j; }}
object load {{ name sink; phases AN; nominal_voltage {voltage}; constant_{suffix}_A {literal(value)}; }}
object line_configuration {{ name c; z11 {literal(z)}; }}
object overhead_line {{ name line; from source; to sink; phases AN; length 1 mile; configuration c; }}
"""

    index = "ABC".index(phase) + 1
    return (
        text.replace("_A", "_" + phase)
        .replace("phases AN", "phases " + phase + "N")
        .replace("z11", f"z{index}{index}")
    )


def expected(z, voltage, power, law, vnom):
    v = complex(voltage)
    for _ in range(1000):
        current = (
            (power / v).conjugate()
            if law == "P"
            else (power / vnom).conjugate() * v / abs(v)
            if law == "I"
            else power.conjugate() * v / vnom**2
        )
        new = voltage - z * current
        if abs(new - v) < 1e-12:
            break
        v = new
    result = {}
    for name, unit, value in [
        ("voltage", "V", new),
        ("current", "A", current),
        ("from.current", "A", current),
        ("to.current", "A", -current),
        ("aux.kcl", "A", 0j),
        ("impedance", "ohm", z),
        ("from.power", "VA", voltage * current.conjugate()),
        ("to.power", "VA", -new * current.conjugate()),
        ("loss", "VA", z * abs(current) ** 2),
    ]:
        add(result, name, unit, value)
    return result


def glm_objects(text, kind):
    result = {}
    for body in re.findall(r"object " + kind + r"\s*\{(.*?)\}", text, re.S):
        name = re.search(r'name\s+"([^"\n]+)";', body)
        if name:
            result[name[1]] = body
    return result


def snapshot(vs, vt, encoded, first, second, kcl, impedance):
    result = {}
    for name, unit, value in [
        ("voltage", "V", vt),
        ("current", "A", encoded),
        ("from.current", "A", first),
        ("to.current", "A", second),
        ("aux.kcl", "A", kcl),
        ("impedance", "ohm", impedance),
        ("from.power", "VA", vs * first.conjugate()),
        ("to.power", "VA", vt * second.conjugate()),
        ("loss", "VA", vs * first.conjugate() + vt * second.conjugate()),
    ]:
        add(result, name, unit, value)
    return result


def native(folder, text, root, aux, phase, *, calibration=False):
    if calibration:
        # Only solver stopping changes; all electrical properties and budgets
        # remain unchanged. Default output is observed separately below.
        text = text.replace("maximum_voltage_error 0;", "maximum_voltage_error 1e-9;")
    # Observe every equivalent terminal, including native load power rather
    # than assuming the generated shunts contribute their declared currents.
    lines = glm_objects(text, "overhead_line")
    loads = glm_objects(text, "load")
    ports = {}
    for side, bus in [("from", "source"), ("to", "sink")]:
        ports[side] = next(
            name
            for name, body in lines.items()
            if f'from "{bus}";' in body and f'to "{aux}";' in body
        )
        ports[side + "_shunt"] = next(
            name for name in loads if f"__{side}_shunt" in name
        )
    model = (
        "clock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:01'; }\n"
        "#set complex_format=%.17g%+.17gj\n#set complex_output_format=RECT\n"
        + text
        + "\nmodule tape;\n"
    )
    outputs = []
    records = [
        ("source", "source", f"voltage_{phase}"),
        ("sink", "sink", f"voltage_{phase}"),
        ("aux", aux, f"voltage_{phase}"),
    ]
    records += [
        (side, ports[side], f"current_in_{phase},current_out_{phase}")
        for side in ["from", "to"]
    ]
    records += [
        (
            side + "_shunt",
            ports[side + "_shunt"],
            f"voltage_{phase},measured_power_{phase}"
            if root[0] == "pnnl"
            else f"voltage_{phase},constant_impedance_{phase},constant_impedance_{phase}N",
        )
        for side in ["from", "to"]
    ]
    for name, bus, properties in records:
        filename = f"{name}.csv"
        outputs.append(filename)
        model += f'object recorder {{ parent "{bus}"; property {properties}; interval 1; file "{filename}"; }}\n'
    (folder / "model.glm").write_text(model)
    run_glm(root[0], folder, root[1], outputs=tuple(outputs))
    values = {name: recorded(folder / f"{name}.csv") for name, _, _ in records}
    vs, vt, encoded = [values[n][f"voltage_{phase}"] for n in ["source", "sink", "aux"]]
    currents = {}
    for side in ["from", "to"]:
        shunt = values[side + "_shunt"]
        if root[0] == "pnnl":
            shunt_current = (
                shunt[f"measured_power_{phase}"] / shunt[f"voltage_{phase}"]
            ).conjugate()
        else:
            # Arras publishes impedance and voltage, but no measured load power.
            shunt_current = sum(
                shunt[f"voltage_{phase}"] / shunt[k]
                for k in [f"constant_impedance_{phase}", f"constant_impedance_{phase}N"]
                if shunt[k] != 0
            )
        currents[side] = values[side][f"current_in_{phase}"] + shunt_current
    # Auxiliary admittance is read from the actual emitted load, not inferred
    # from the expected impedance or substituted for measured physical ports.
    aux_load = next(
        body
        for name, body in loads.items()
        if name == aux or f'parent "{aux}";' in body
    )
    impedance = re.search(rf"constant_impedance_{phase}[N]? ([^;]+);", aux_load)
    if not impedance:
        raise ValueError("auxiliary impedance missing")

    yaux = 1 / parse_complex(impedance[1])
    kcl = (
        -sum(values[s][f"current_out_{phase}"] for s in ["from", "to"]) + yaux * encoded
    )
    return snapshot(vs, vt, encoded, currents["from"], currents["to"], kcl, -yaux)


def dss_snapshot(path, aux):
    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("DSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    e.Text.Command("set tolerance=1e-12 maxiterations=1000 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged():
        raise ValueError("DSS convergence failure")

    def voltage(bus):
        e.Circuit.SetActiveBus(bus)
        values = e.Bus.Voltages()
        return complex(values[0], values[1])

    vs, vt, encoded = voltage("source"), voltage("sink"), voltage(aux)
    ports = {"source": 0j, "sink": 0j, aux: 0j}
    yaux = 0j
    for name in e.Circuit.AllElementNames():
        e.Circuit.SetActiveElement(name)
        buses = [b.split(".")[0].lower() for b in e.CktElement.BusNames()]
        equivalent = (name.lower().startswith("line.") and aux in buses) or (
            name.lower().startswith("load.")
            and ("shunt" in name.lower() or buses[0] == aux)
        )
        if not equivalent:
            continue
        if name.lower().startswith("load.") and buses[0] == aux:
            raw_y = e.CktElement.YPrim()
            yaux += complex(raw_y[0], raw_y[1])
        n = e.CktElement.NumConductors()
        raw = e.CktElement.Currents()
        for terminal, bus in enumerate(buses):
            if bus in ports:
                offset = 2 * terminal * n
                ports[bus] += complex(raw[offset], raw[offset + 1])
    return snapshot(vs, vt, encoded, ports["source"], ports["sink"], ports[aux], -yaux)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--dss-bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profile", choices=["pnnl", "arras"], action="append")
    parser.add_argument("--law", choices=["Z", "P", "I"], action="append")
    parser.add_argument("--phase", choices=list("ABC"), default="A")
    parser.add_argument("--impedance", type=complex, default=(1 + 1j) * 6.25e-7)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(
        engines=LOCK,
        phase=args.phase,
        impedance=[args.impedance.real, args.impedance.imag],
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        dss_bridge_sha256=hashlib.sha256(args.dss_bridge.read_bytes()).hexdigest(),
        calibration_absolute_stopping_V=1e-9,
        default_output_budgets=FEEDER_TOLERANCES,
        cases={},
    )
    for law in args.law or ["Z", "P", "I"]:
        for profile in args.profile or ["pnnl", "arras"]:
            folder = args.output / law / profile
            folder.mkdir(parents=True)
            z = args.impedance
            voltage = 7200
            source = folder / "input.glm"
            source.write_text(deck(z, voltage, law, args.phase))
            row = {"passed": False, "comparisons": {}}
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
                for stage in [
                    "fresh",
                    "restored",
                    "edited",
                    "rebased",
                    "grounded_reference",
                ]:
                    node = data[stage]["network"]
                    node = node.get("value", node)
                    node = node.get("network", node)
                    line = node["lines"][0]
                    code = next(
                        c for c in node["linecodes"] if c["name"] == line["linecode"]
                    )
                    zz = (
                        complex(code["r_series"][0][0], code["x_series"][0][0])
                        * line["length"]
                    )
                    load = next(
                        entry for entry in node["loads"] if entry["bus"] == "sink"
                    )
                    p = complex(load["p_nom"][0], load["q_nom"][0])
                    model = load["voltage_model"]
                    base = model["v_nom"][0] if model["v_nom"] else voltage
                    wanted = expected(
                        zz,
                        node["sources"][0].get("value", node["sources"][0])[
                            "v_magnitude"
                        ][0],
                        p,
                        law,
                        base,
                    )
                    canonical_path = emitted / f"{stage}.glm"
                    aux = next(
                        name
                        for name in glm_objects(canonical_path.read_text(), "load")
                        if name.endswith("__glm_current")
                    )
                    repeated = folder / stage / "reimported"
                    command(
                        [
                            str(args.bridge.resolve()),
                            str(canonical_path.resolve()),
                            str(repeated.resolve()),
                            profile,
                        ],
                        folder,
                    )
                    for variant, path in [
                        ("canonical", canonical_path),
                        ("reimported", repeated / "fresh.glm"),
                        ("reimported_ir", repeated / "restored.glm"),
                    ]:
                        run = folder / stage / ("native_" + variant)
                        run.mkdir(parents=True)
                        actual = native(
                            run,
                            path.read_text(),
                            (profile, args.pnnl_root),
                            aux,
                            args.phase,
                        )
                        row["comparisons"][f"{stage}/{variant}/default"] = compare(
                            wanted, actual, tolerances=FEEDER_TOLERANCES
                        )
                        refined = folder / stage / ("calibration_" + variant)
                        refined.mkdir()
                        actual = native(
                            refined,
                            path.read_text(),
                            (profile, args.pnnl_root),
                            aux,
                            args.phase,
                            calibration=True,
                        )
                        row["comparisons"][f"{stage}/{variant}/calibration"] = compare(
                            wanted, actual
                        )
                    for variant, path, current_bus in [
                        ("dss", emitted / f"{stage}.dss/case.dss", "_pio_current_line"),
                        ("glm_reimported_dss", repeated / "fresh.dss/case.dss", aux),
                    ]:
                        row["comparisons"][f"{stage}/{variant}"] = compare(
                            wanted, dss_snapshot(path, current_bus)
                        )
                    dss_repeated = folder / stage / "dss_reimported"
                    result = json.loads(
                        command(
                            [
                                str(args.dss_bridge.resolve()),
                                str((emitted / f"{stage}.dss/case.dss").resolve()),
                                str(dss_repeated.resolve()),
                            ],
                            folder,
                        )
                    )
                    if (
                        not result["ir_network_equal"]
                        or not result["targets"]["dss"]["ok"]
                    ):
                        raise ValueError("DSS reimport/IR/emission failed")
                    row["comparisons"][f"{stage}/dss_reimported"] = compare(
                        wanted,
                        dss_snapshot(
                            dss_repeated / "dss/case.dss", "_pio_current_line"
                        ),
                    )
                    if stage == "fresh":
                        ideal = dict(wanted)
                        ideal["impedance"] = {"unit": "ohm", "value": [0.0, 0.0]}
                        row["ideal_contact_negative"] = compare(ideal, actual)
                    if stage == "edited":
                        row["unchanged_negative"] = compare(
                            expected(
                                z,
                                voltage,
                                complex(0, -432000)
                                if law == "Z"
                                else complex(1000, 200)
                                if law == "P"
                                else complex(7200, 1440),
                                law,
                                voltage,
                            ),
                            actual,
                        )
                row["passed"] = (
                    all(v["passed"] for v in row["comparisons"].values())
                    and not row["unchanged_negative"]["passed"]
                    and not row["ideal_contact_negative"]["passed"]
                )
            except Exception as e:
                row["error"] = str(e)
            report["cases"][f"{law}/{profile}"] = row
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
            print(
                law,
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
    report["passed"] = all(v["passed"] for v in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

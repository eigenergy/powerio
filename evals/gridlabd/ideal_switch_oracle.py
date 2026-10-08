"""Qualify exact FBS contact output, including grouping and source orientation.

The reference solves independent linear or scalar ZIP equations. No epsilon
contact substitutes for the ideal constraint. A finite-contact negative control
must disagree. Reversed input links intentionally require canonical orientation;
the forward native baseline is solved separately. All values use SI units.
"""

import argparse
import cmath
import hashlib
import json
import math
import re
from pathlib import Path

import numpy as np
from oracle import add, command, compare, literal, recorded, run_glm


def variants():
    return [
        ("single_a", "A", "A", False, False, False),
        ("single_c_zip", "C", "C", True, False, False),
        ("phase_ac", "AC", "AC", False, False, False),
        ("three_zip", "ABC", "ABC", True, False, False),
        ("mixed_z", "ABC", "AC", False, False, False),
        ("mixed_zip", "ABC", "AC", True, False, False),
        ("coupled", "ABC", "ABC", False, True, False),
        ("coupled_mixed", "ABC", "AC", False, True, False),
        ("coupled_capacitance", "ABC", "AC", False, "capacitance", False),
        ("commuting_capacitance", "ABC", "AC", False, "commuting_capacitance", False),
        ("reversed_zip", "ABC", "AC", True, False, True),
    ]


def parameters(phases, closed, zip_load, coupled, edited=False):
    angle = np.array([cmath.rect(1, -"ABC".index(p) * 2 * math.pi / 3) for p in phases])
    source = 240 * angle
    series = np.diag([1 + "ABC".index(p) * 0.2 for p in phases]).astype(complex)
    if coupled:
        series += (np.ones(series.shape) - np.eye(len(phases))) * (0.1 + 0.05j)
    loads = np.array([40 + "ABC".index(p) * 10 for p in phases], dtype=float)
    active = np.array([p in closed for p in phases])
    powers = 500 * active if zip_load else np.zeros(len(phases))
    currents = 1.5 * active if zip_load else np.zeros(len(phases))
    if edited:
        source[0] *= 1.03  # The Rust bridge edits the first source terminal.
        series *= 1.1
        loads /= 1.25
        powers = powers * 1.25
        currents = currents * 1.25
    return source, series, loads, powers, currents, active, angle


def reference(spec, edited=False, epsilon=0):
    _, phases, closed, zip_load, coupled, _ = spec
    source, z, loads, power, amps, active, angle = parameters(
        phases, closed, zip_load, coupled, edited
    )
    current = np.zeros(len(phases), dtype=complex)
    shunt = end_shunt(len(phases), coupled, edited)
    if zip_load:
        if coupled:
            raise ValueError("scalar ZIP reference requires diagonal series impedance")
        for i in np.flatnonzero(active):
            r = z[i, i].real + epsilon
            a = 1 + r / loads[i]
            b = abs(source[i]) - r * amps[i]
            voltage = (b + math.sqrt(b * b - 4 * a * r * power[i])) / (2 * a)
            current[i] = (power[i] / voltage + amps[i] + voltage / loads[i]) * angle[i]
        middle = source - z @ current
    else:
        series_y = np.linalg.inv(z)
        load_y = active / (loads + epsilon)
        middle = np.linalg.solve(series_y + shunt + np.diag(load_y), series_y @ source)
        current = load_y * middle
    upstream_in = np.linalg.solve(z, source - middle) + shunt @ source
    sink = np.where(active, middle - epsilon * current, 0j)
    result = {}
    for bus, voltages in (("source", source), ("middle", middle), ("sink", sink)):
        for p, value in zip(phases, voltages, strict=True):
            add(result, f"{bus}.{p}", "V", value)
    for branch, vin, vout, iin in (
        ("upstream", source, middle, upstream_in),
        ("contact", middle, sink, current),
    ):
        for i, p in enumerate(phases):
            for side, values, amps_side, sign in (
                ("in", vin, iin, 1),
                ("out", vout, current, -1),
            ):
                add(result, f"{branch}.current_{side}_{p}", "A", sign * amps_side[i])
                add(
                    result,
                    f"{branch}.power_{side}_{p}",
                    "VA",
                    sign * values[i] * amps_side[i].conjugate(),
                )
        add(
            result,
            branch + ".loss",
            "VA",
            sum(vin * iin.conjugate() - vout * current.conjugate()),
        )
    return result


def end_shunt(n, coupled, edited=False):
    if coupled == "commuting_capacitance":
        return 1j * np.eye(n) * 0.0003 * (1.1 if edited else 1)
    if coupled != "capacitance":
        return np.zeros((n, n), dtype=complex)
    return (
        1j
        * (np.eye(n) * 0.0003 - (np.ones((n, n)) - np.eye(n)) * 0.00004)
        * (1.1 if edited else 1)
    )


def deck(spec):
    _, phases, closed, zip_load, coupled, reverse = spec
    source, z, loads, powers, amps, _, angle = parameters(
        phases, closed, zip_load, coupled
    )
    text = "module powerflow { solver_method FBS; line_capacitance TRUE; }\n"
    text += f"object node {{ name source; phases {phases}N; bustype SWING; nominal_voltage 240; "
    text += (
        " ".join(
            f"voltage_{p} {literal(v)};" for p, v in zip(phases, source, strict=True)
        )
        + " }\n"
    )
    text += f"object node {{ name middle; phases {phases}N; nominal_voltage 240; maximum_voltage_error 1e-10; }}\n"
    text += f"object load {{ name sink; phases {phases}N; nominal_voltage 240; maximum_voltage_error 1e-10; "
    for i, p in enumerate(phases):
        text += f"constant_impedance_{p} {literal(loads[i])}; "
        if zip_load and p in closed:
            text += f"constant_power_{p} {literal(powers[i])}; constant_current_{p} {literal(amps[i] * angle[i])}; "
    text += "}\nobject line_configuration { name config; "
    shunt = end_shunt(len(phases), coupled)
    for i, p in enumerate(phases):
        for j, q in enumerate(phases):
            text += f"z{'ABC'.index(p) + 1}{'ABC'.index(q) + 1} {literal(z[i, j] * 1609.344)}; "
            if coupled in ("capacitance", "commuting_capacitance"):
                c = shunt[i, j].imag * 1609.344 / (math.pi * 60 * 1e-9)
                text += f"c{'ABC'.index(p) + 1}{'ABC'.index(q) + 1} {c:.15g}; "
    text += "}\n"
    line_ends = "middle; to source" if reverse else "source; to middle"
    contact_ends = "sink; to middle" if reverse else "middle; to sink"
    text += f"object overhead_line {{ name upstream; phases {phases}N; from {line_ends}; length 1 m; configuration config; }}\n"
    text += f"object switch {{ name contact; phases {phases}N; from {contact_ends}; operating_mode INDIVIDUAL; switch_impedance 0+0j; latitude 0; "
    text += (
        " ".join(
            f"phase_{p}_state {'CLOSED' if p in closed else 'OPEN'};" for p in phases
        )
        + " }\n"
    )
    return text


def instrumentation(text, phases):
    matches = re.findall(r"object switch\s*\{\s*name\s+([^;]+);", text)
    if len(matches) != 1:
        raise ValueError("expected one recombined native contact")
    contact = matches[0].strip().strip('"')
    extra = "\nmodule tape;\n#set complex_format=%.15g%+.15g%c\n#set complex_output_format=DEFAULT\nclock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:02'; }\n"
    outputs = []
    for bus in ("source", "middle", "sink"):
        extra += (
            f"object recorder {{ parent {bus}; property "
            + ",".join("voltage_" + p for p in phases)
            + f'; interval 1; file "{bus}.csv"; }}\n'
        )
        outputs.append(bus + ".csv")
    for label, parent in (("upstream", "upstream"), ("contact", contact)):
        props = ",".join(
            f"{q}_{p}"
            for p in phases
            for q in ("current_in", "current_out", "power_in", "power_out")
        )
        extra += f'object recorder {{ parent {parent}; property {props},power_losses; interval 1; file "{label}.csv"; }}\n'
        outputs.append(label + ".csv")
    return text + extra, tuple(outputs)


def snapshot(directory, phases):
    result = {}
    for bus in ("source", "middle", "sink"):
        row = recorded(directory / (bus + ".csv"))
        for p in phases:
            add(result, f"{bus}.{p}", "V", row["voltage_" + p])
    for branch in ("upstream", "contact"):
        row = recorded(directory / (branch + ".csv"))
        for p in phases:
            for side, sign in (("in", 1), ("out", -1)):
                for quantity, unit in (("current", "A"), ("power", "VA")):
                    key = f"{quantity}_{side}_{p}"
                    add(result, f"{branch}.{key}", unit, sign * row[key])
        add(result, branch + ".loss", "VA", row["power_losses"])
    return result


def run_case(args, profile, directory, spec, text, edited=False):
    directory.mkdir()
    data, outputs = instrumentation(text, spec[1])
    (directory / "model.glm").write_text(data)
    result = {
        "passed": False,
        "input_sha256": hashlib.sha256(data.encode()).hexdigest(),
    }
    try:
        result["version"] = run_glm(profile, directory, args.pnnl_root, outputs)
        actual = snapshot(directory, spec[1])
        expected = reference(spec, edited)
        result["comparison"] = compare(expected, actual)
        result["epsilon_negative_control"] = compare(
            reference(spec, edited, 1e-4), actual
        )
        result["passed"] = (
            result["comparison"]["passed"]
            and not result["epsilon_negative_control"]["passed"]
        )
        if spec[4] == "capacitance":
            z = parameters(*spec[1:5], edited)[1]
            y = end_shunt(len(spec[1]), spec[4], edited)
            current = np.array(
                [complex(*actual[f"contact.current_in_{p}"]["value"]) for p in spec[1]]
            )
            observed = np.array(
                [
                    complex(*actual[f"upstream.current_in_{p}"]["value"])
                    - complex(*expected[f"upstream.current_in_{p}"]["value"])
                    for p in spec[1]
                ]
            )
            predicted = (z @ y - y @ z) @ current
            residual = float(np.max(np.abs(observed - predicted)))
            result["native_d_equals_a_witness"] = {
                "residual_amperes": residual,
                "matched": residual < 1e-10,
            }
    except Exception as error:
        result["error"] = str(error)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--case", action="append", choices=[v[0] for v in variants()])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    for profile in ("pnnl", "arras"):
        for spec in variants():
            if args.case and spec[0] not in args.case:
                continue
            key = profile + "/" + spec[0]
            directory = args.output / key
            directory.mkdir(parents=True)
            forward = (*spec[:-1], False)
            inputs = directory / "input.glm"
            inputs.write_text(deck(spec))
            baseline = run_case(
                args, profile, directory / "native", forward, deck(forward)
            )
            report["cases"][key + "/native"] = baseline
            # Keep native discrepancies visible, then check canonical output
            # independently. A native failure cannot qualify a conversion.
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
            try:
                decoded = json.loads(
                    command(
                        [
                            str(args.bridge.resolve()),
                            str(inputs.resolve()),
                            str(directory.resolve()),
                            profile,
                        ],
                        directory,
                    )
                )
            except RuntimeError as error:
                report["cases"][key + "/emission"] = {
                    "passed": False,
                    "error": str(error),
                }
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(key, "emission FAILED", flush=True)
                continue
            if (directory / "fresh.glm").read_bytes() != (
                directory / "restored.glm"
            ).read_bytes():
                raise ValueError("IR changed canonical ideal contacts")
            for variant in (
                "fresh",
                "restored",
                "edited",
                "rebased",
                "grounded_reference",
            ):
                result = run_case(
                    args,
                    profile,
                    directory / variant,
                    spec,
                    (directory / (variant + ".glm")).read_text(),
                    variant == "edited",
                )
                result["diagnostics"] = decoded[variant]["diagnostics"]
                result["native_baseline_passed"] = baseline["passed"]
                report["cases"][key + "/" + variant] = result
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(
                    key, variant, "passed" if result["passed"] else "FAILED", flush=True
                )
    return 0 if all(v["passed"] for v in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())

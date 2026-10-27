"""Calibrate overhead conductor primitives, explicit neutral reduction and ports.

These are native-engine/reference checks, not PowerIO geometry conversion.
GridLAB-D's rounded modified-Carson constants and OpenDSS SimpleCarson's SI
constants are tested separately; their differences must not be hidden by a
loose cross-engine tolerance. Cross-section coordinates are feet, not map data.
"""

import argparse
import hashlib
import json
import math
import xml.etree.ElementTree as ET
from pathlib import Path

import numpy as np
from oracle import LOCK, add, compare, literal, parse_complex, read_glm, run_glm
from roundtrip_oracle import instrumentation

# x, height (ft), GMR (ft), AC resistance (ohm/mile), diameter (in).
WIRES = {
    "A": (-4, 32, 0.0244, 0.306, 0.721),
    "B": (0, 34, 0.030, 0.250, 0.800),
    "C": (5, 31, 0.020, 0.400, 0.650),
    "N": (1, 26, 0.00814, 0.592, 0.563),
}
MILE = 1609.344


def primitives(flags, frequency=60, engine="glm"):
    """Full unreduced Z (ohm/mile) and Y (S/mile), phase/neutral order."""
    n = len(flags)
    z = np.zeros((n, n), complex)
    potential = np.zeros((n, n))
    for i, p in enumerate(flags):
        x, height, gmr, resistance, diameter = WIRES[p]
        for j, q in enumerate(flags):
            xj, hj, _, _, _ = WIRES[q]
            distance = math.hypot(x - xj, height - hj)
            spacing = gmr if i == j else distance
            if engine == "glm":
                earth = 7.6786 + math.log(100 / frequency) / 2
                z[i, j] = 0.00158836 * frequency + 1j * 0.00202237 * frequency * (
                    earth - math.log(spacing)
                )
            else:
                # Preserve the pinned DSS source constants, including rounding.
                wmu = 6.283185307 * frequency * 12.56637e-7
                z[i, j] = MILE * (
                    wmu / 8
                    + 1j
                    * wmu
                    / 6.283185307
                    * math.log(658.5 * math.sqrt(100 / frequency) / (spacing * 0.3048))
                )
            if i == j:
                z[i, j] += resistance
                potential[i, j] = math.log(2 * height / (diameter / 24))
            else:
                potential[i, j] = math.log(math.hypot(x - xj, height + hj) / distance)
    permittivity = 0.014240e-6 if engine == "glm" else 8.854e-12 * MILE
    y = (
        1j
        * 2
        * math.pi
        * frequency
        * 2
        * math.pi
        * permittivity
        * np.linalg.inv(potential)
    )
    return z, y


def reduced(flags, frequency=60, engine="glm"):
    z, y = primitives(flags, frequency, engine)
    if flags.endswith("N"):
        z = z[:-1, :-1] - np.outer(z[:-1, -1], z[-1, :-1]) / z[-1, -1]
        # Neutral potential is zero for this reduced circuit. Its capacitive
        # current may be nonzero: take the phase block of full shunt Y.
        y = y[:-1, :-1]
    return z, y


def deck(flags, frequency, capacitance, solver):
    phases = flags.replace("N", "")
    text = [
        "// Synthetic PowerIO conductor geometry calibration; no controls.",
        "#set complex_format=%.15g%+.15gj",
        "#set complex_output_format=RECT",
        "clock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:02'; }",
        f"module powerflow {{ solver_method {solver}; nominal_frequency {frequency}; line_capacitance {'TRUE' if capacitance else 'FALSE'}; }}",
        "module tape;",
        f"object node {{ name source; phases {flags}; bustype SWING; nominal_voltage 240;",
    ]
    for p in phases:
        angle = [0, -2 * math.pi / 3, 2 * math.pi / 3]["ABC".index(p)]
        text.append(f"voltage_{p} {literal(240 * np.exp(1j * angle))};")
    text += [
        "}",
        f"object load {{ name sink; phases {flags}; nominal_voltage 240; maximum_voltage_error 1e-10;",
    ]
    for p in phases:
        text.append(f"constant_impedance_{p} {literal(loads()[ord(p) - ord('A')])};")
    text.append("}")
    for p in flags:
        _, _, gmr, r, diameter = WIRES[p]
        text.append(
            f"object overhead_line_conductor {{ name wire_{p}; geometric_mean_radius {gmr} ft; resistance {r} Ohm/mile; diameter {diameter} in; }}"
        )
    text.append("object line_spacing { name spacing;")
    for i, p in enumerate(flags):
        x, h, *_ = WIRES[p]
        text.append(f"distance_{p}E {h} ft;")
        for q in flags[i + 1 :]:
            xx, hh, *_ = WIRES[q]
            text.append(f"distance_{p}{q} {math.hypot(x - xx, h - hh):.17g} ft;")
    text += ["}", "object line_configuration { name code; spacing spacing;"]
    text += [f"conductor_{p} wire_{p};" for p in flags]
    text += [
        "}",
        f"object overhead_line {{ name line; phases {flags}; from source; to sink; length 1 mile; configuration code; }}",
    ]
    return "\n".join(text) + "\n" + instrumentation("ABC")


def loads():
    return np.array([20 + 5j, 35 + 12j, 15 + 3j])


def reference(
    flags,
    frequency,
    capacitance,
    omit_neutral=False,
    native_readback=False,
    edited=False,
):
    phases = flags.replace("N", "")
    idx = ["ABC".index(p) for p in phases]
    z, y = reduced(phases if omit_neutral else flags, frequency)
    if not capacitance:
        y *= 0
    source = 240 * np.exp(1j * np.array([0, -2 * math.pi / 3, 2 * math.pi / 3]))[idx]
    demand = loads()[idx].copy()
    if edited:
        source[0] *= 1.03
        z *= 1.1
        y *= 1.1
        demand /= 1.25
    series = np.linalg.inv(z)
    sink = np.linalg.solve(series + y / 2 + np.diag(1 / demand), series @ source)
    current = series @ (source - sink)
    cin, cout = current + y @ source / 2, -current + y @ sink / 2
    if native_readback:
        # Published link currents use d=a=I+ZY/2, instead of physical I+YZ/2.
        cin = (y + y @ z @ y / 4) @ sink + (np.eye(len(phases)) + z @ y / 2) @ (-cout)
    result = {}
    for i, p in enumerate(phases):
        add(result, f"source.{p}.voltage", "V", source[i])
        add(result, f"sink.{p}.voltage", "V", sink[i])
        for side, v, amps in [("from", source[i], cin[i]), ("to", sink[i], cout[i])]:
            add(result, f"line.{side}.{p}.current", "A", amps)
            add(result, f"line.{side}.{p}.power", "VA", v * amps.conjugate())
        for j, q in enumerate(phases):
            add(result, f"line.z.{p}{q}", "ohm", z[i, j])
            add(result, f"line.y.{p}{q}", "S", y[i, j])
    losses = source * cin.conjugate() + sink * cout.conjugate()
    if native_readback:
        # Native reporting makes each real phase loss positive before summing.
        losses = abs(losses.real) + 1j * losses.imag
    add(result, "line.loss", "VA", sum(losses))
    residual = float(max(abs(cout + sink / demand)))
    assert residual < 1e-9
    return result, residual


def snapshot(path, flags):
    phases = flags.replace("N", "")
    raw = read_glm(path, 3)
    expected, _ = reference(flags, 60, False)
    result = {k: v for k, v in raw.items() if k in expected}
    link = ET.parse(path / "impedance.xml").find("overhead_line")

    def matrix(name):
        return np.array(
            [
                [
                    parse_complex(
                        link.findtext(f"{name}_matrix/{name}{ord(p) - 64}{ord(q) - 64}")
                    )
                    for q in phases
                ]
                for p in phases
            ]
        )

    z = matrix("b")
    y = 2 * np.linalg.solve(z, matrix("a") - np.eye(len(phases)))
    for i, p in enumerate(phases):
        for j, q in enumerate(phases):
            add(result, f"line.y.{p}{q}", "S", y[i, j])
    return result


def dss_matrices(flags, frequency, reduce):
    import opendssdirect as dss

    engine = dss.NewContext()
    assert LOCK["dss"]["version"] in engine.Basic.Version()
    commands = ["clear", "new circuit.geometry", "set earthmodel=Carson"]
    for p in flags:
        _, _, gmr, r, diameter = WIRES[p]
        commands.append(
            f"new wiredata.w{p} rac={r} rdc={r} runits=mi gmrac={gmr} gmrunits=ft radius={diameter / 2} radunits=in"
        )
    nph = len(flags.replace("N", ""))
    commands.append(
        f"new linegeometry.g nconds={len(flags)} nphases={nph} reduce={'yes' if reduce else 'no'}"
    )
    for i, p in enumerate(flags):
        x, h, *_ = WIRES[p]
        commands.append(f"~ cond={i + 1} wire=w{p} x={x} h={h} units=ft")
    nodes = ".".join(str(i + 1) for i in range(nph))
    if not reduce and flags.endswith("N"):
        nodes += ".0"
    # The API uses ActiveEarthModel; setting the default alone does not update
    # that state. Calculating this line activates the explicitly chosen model.
    commands += [
        f"new line.probe bus1=sourcebus.{nodes} bus2=sink.{nodes} geometry=g earthmodel=Carson length=1 units=mi",
        "set controlmode=off",
        "solve",
    ]
    for text in commands:
        engine.Text.Command(text)
    if not engine.Solution.Converged():
        raise ValueError("DSS geometry activation circuit did not converge")
    engine.LineGeometries.Name("g")
    n = nph if reduce else len(flags)
    z = np.array(engine.LineGeometries.Rmatrix(frequency, 1, 1)).reshape(
        n, n
    ) + 1j * np.array(engine.LineGeometries.Xmatrix(frequency, 1, 1)).reshape(n, n)
    y = (
        1j
        * 2
        * math.pi
        * frequency
        * 1e-9
        * np.array(engine.LineGeometries.Cmatrix(frequency, 1, 1)).reshape(n, n)
    )
    expected_z, expected_y = (reduced if reduce else primitives)(
        flags, frequency, "dss"
    )
    expected, actual = {}, {}
    for i in range(n):
        for j in range(n):
            for key, unit, a, b in [
                ("z", "ohm", expected_z[i, j], z[i, j]),
                ("y", "S", expected_y[i, j], y[i, j]),
            ]:
                add(expected, f"{key}.{i}.{j}", unit, a)
                add(actual, f"{key}.{i}.{j}", unit, b)
    return {
        "version": engine.Basic.Version(),
        "deck": "\n".join(commands),
        "comparison": compare(expected, actual),
        "observed": actual,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument(
        "--native-evidence",
        type=Path,
        help="Recheck existing native outputs after verifying input hashes",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "native": {}, "dss": {}}
    previous = None
    if args.native_evidence:
        evidence = args.native_evidence / "report.json"
        previous = json.loads(evidence.read_text())
        report["native_evidence_sha256"] = hashlib.sha256(
            evidence.read_bytes()
        ).hexdigest()
    cases = [(flags, 60) for flags in ("A", "AN", "BC", "BCN", "ABC", "ABCN")] + [
        ("ABCN", 50)
    ]

    def save():
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")

    for flags, f in cases:
        for reduce in (False, True):
            key = f"{flags}/{f}/reduce_{reduce}"
            result = dss_matrices(flags, f, reduce)
            report["dss"][key] = result
            save()
            print("dss", key, result["comparison"]["passed"], flush=True)
        for profile in ("pnnl", "arras"):
            for solver, cap in (("NR", False), ("NR", True), ("FBS", False)):
                key = f"{profile}/{flags}/{f}/{solver}/cap_{cap}"
                path = args.output / key
                path.mkdir(parents=True)
                text = deck(flags, f, cap, solver)
                (path / "model.glm").write_text(text)
                result = {
                    "passed": False,
                    "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
                }
                try:
                    if previous is None:
                        result["version"] = run_glm(profile, path, args.pnnl_root)
                    else:
                        path = args.native_evidence / key
                        old = previous["native"][key]
                        assert old["input_sha256"] == result["input_sha256"]
                        assert (path / "model.glm").read_text() == text
                        result["version"] = old["version"]
                        assert LOCK[profile]["version"] in result["version"]
                    result["engine_directory"] = str(path.resolve())
                    actual = snapshot(path, flags)
                    expected, residual = reference(flags, f, cap)
                    result["comparison"] = compare(expected, actual)
                    result["native_readback_equations"] = compare(
                        reference(flags, f, cap, native_readback=True)[0], actual
                    )
                    result["kcl_residual_A"] = residual
                    result["passed"] = result["comparison"]["passed"]
                    if flags.endswith("N"):
                        result["neutral_omission_negative_control"] = compare(
                            reference(flags, f, cap, True)[0], actual
                        )
                        result["passed"] &= not result[
                            "neutral_omission_negative_control"
                        ]["passed"]
                except Exception as error:
                    result["error"] = str(error)
                report["native"][key] = result
                save()
                print(key, result["passed"], flush=True)
    return (
        0
        if all(v["passed"] for v in report["native"].values())
        and all(v["comparison"]["passed"] for v in report["dss"].values())
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())

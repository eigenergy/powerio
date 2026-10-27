"""Validate cable geometry, native reduction and canonical/edited GLM circuits.

CN606/CN607 use the small conductor parameter sets in the pinned IEEE13 case.
The reference eliminates the full return block with NumPy, independently of
PowerIO's successive Schur complements. Root-meter currents witness physical
ports separately from the known native link-current/loss reporting differences.
"""

import argparse
import hashlib
import json
import math
import xml.etree.ElementTree as ET
from pathlib import Path

import numpy as np
from oracle import (
    add,
    command,
    compare,
    literal,
    parse_complex,
    read_glm,
    recorded,
    run_glm,
)
from overhead_geometry_oracle import loads
from overhead_roundtrip_oracle import physical_snapshot
from roundtrip_oracle import instrumentation
from shunt_measurement import instrument

MILE = 1609.344
# Outer diameter(in), conductor GMR(ft), diameter(in), R(ohm/mile),
# strand GMR(ft), R(ohm/mile), diameter(in), count.
CN = dict(
    CN606=(1.29, 0.0171, 0.567, 0.41, 0.00208, 14.872, 0.0640837, 13),
    CN607=(1.06, 0.0111, 0.368, 0.97, 0.0111, 0.97, 0.0640837, 6),
)


def properties(kind):
    outer, gmr, diameter, resistance, ngmr, nr, nd, count = CN.get(kind, CN["CN606"])
    result = dict(
        outer_diameter=outer,
        conductor_gmr=gmr,
        conductor_diameter=diameter,
        conductor_resistance=resistance,
        insulation_relative_permitivitty=2.3,
    )
    if kind.startswith("CN"):
        result.update(
            neutral_gmr=ngmr,
            neutral_resistance=nr,
            neutral_diameter=nd,
            neutral_strands=count,
        )
    elif kind == "TS":
        result.update(
            shield_gmr=0.045,
            shield_resistance=1.7,
            shield_diameter=1.1,
            shield_thickness=0.005,
        )
    return result


def distances(kind):
    return dict(
        AB=0.5, BC=0.5, AC=1.0, AN=1 / 12 if kind == "CN607" else 0.8, BN=0.9, CN=1.1
    )


def matrices(kind, flags, frequency, capacitance, omit_return=False, profile="pnnl"):
    p = properties(kind)
    phases = flags.replace("N", "")
    n = len(phases)
    wires = [
        (phase, p["conductor_gmr"], p["conductor_resistance"], 0.0) for phase in phases
    ]
    cap = 0.0
    if kind.startswith("CN"):
        count = p["neutral_strands"]
        radius = (p["outer_diameter"] - p["neutral_diameter"]) / 24
        gmr = (p["neutral_gmr"] * count * radius ** (count - 1)) ** (1 / count)
        r = p["neutral_resistance"] / count
        denominator = (
            math.log(radius / (p["conductor_diameter"] / 24))
            - math.log(count * p["neutral_diameter"] / 24 / radius) / count
        )
        cap = 2 * math.pi * 0.01420e-6 * 2.3 / denominator
        if not omit_return:
            wires.extend((phase, gmr, r, radius) for phase in phases)
    elif kind == "TS":
        cap = (
            2
            * math.pi
            * 0.01420e-6
            * 2.3
            / math.log(
                (p["shield_diameter"] - p["shield_thickness"]) / p["conductor_diameter"]
            )
        )
        if not omit_return:
            wires.extend(
                (phase, p["shield_gmr"], p["shield_resistance"], p["shield_gmr"])
                for phase in phases
            )
    if flags.endswith("N") and not omit_return:
        if profile == "pnnl":
            wires.append(("N", p["conductor_gmr"], p["conductor_resistance"], 0.0))
        elif kind == "TS":
            wires.append(
                ("N", p.get("neutral_gmr", 0.0), p.get("neutral_resistance", 0.0), 0.0)
            )
    earth = math.log(100 / frequency) / 2 + 7.6786
    z = np.empty((len(wires), len(wires)), complex)
    for i, (a, gmr, resistance, ring_a) in enumerate(wires):
        for j, (b, _, _, ring_b) in enumerate(wires):
            if i == j and gmr == 0:
                z[i, j] = 0
                continue
            self_resistance = resistance * (
                1.1 if kind == "BARE" and omit_return else 1.0
            )
            distance = (
                gmr
                if i == j
                else max(ring_a, ring_b)
                if a == b
                else distances(kind)["".join(sorted((a, b)))]
            )
            z[i, j] = (
                (self_resistance if i == j else 0)
                + 0.00158836 * frequency
                + 1j * 0.00202237 * frequency * (math.log(1 / distance) + earth)
            )
    reduced = z[:n, :n]
    if len(wires) > n:
        reduced = reduced - z[:n, n:] @ np.linalg.solve(z[n:, n:], z[n:, :n])
    return reduced, np.eye(n) * 1j * 2 * math.pi * frequency * (
        cap if capacitance else 0
    )


def reference(
    kind, flags, frequency, capacitance, edited=False, omit_return=False, profile="pnnl"
):
    phases = flags.replace("N", "")
    indices = ["ABC".index(p) for p in phases]
    z, y = matrices(kind, flags, frequency, capacitance, omit_return, profile)
    source = (
        240 * np.exp(1j * np.array([0, -2 * math.pi / 3, 2 * math.pi / 3]))[indices]
    )
    demand = loads()[indices].copy()
    if edited:
        source[0] *= 1.03
        demand /= 1.25
        z *= 1.1
        y *= 1.1
    series = np.linalg.inv(z)
    sink = np.linalg.solve(series + y / 2 + np.diag(1 / demand), series @ source)
    amps = series @ (source - sink)
    cin, cout = amps + y @ source / 2, -amps + y @ sink / 2
    result = {}
    for i, p in enumerate(phases):
        add(result, f"source.{p}.voltage", "V", source[i])
        add(result, f"sink.{p}.voltage", "V", sink[i])
        for side, v, current in [("from", source[i], cin[i]), ("to", sink[i], cout[i])]:
            add(result, f"line.{side}.{p}.current", "A", current)
            add(result, f"line.{side}.{p}.power", "VA", v * current.conjugate())
        for j, q in enumerate(phases):
            add(result, f"line.z.{p}{q}", "ohm", z[i, j])
            add(result, f"line.y.{p}{q}", "S", y[i, j])
    add(
        result,
        "line.loss",
        "VA",
        sum(source * cin.conjugate() + sink * cout.conjugate()),
    )
    assert max(abs(cout + sink / demand)) < 1e-9
    return result


def deck(kind, flags, frequency, capacitance, solver="NR"):
    phases = flags.replace("N", "")
    text = [
        "#set iteration_limit=100000;",
        f"module powerflow {{ solver_method {solver}; nominal_frequency {frequency}; line_capacitance {str(capacitance).lower()}; }}",
        f'object meter {{ name "source"; phases {flags}; bustype SWING; nominal_voltage 240;',
    ]
    for p in phases:
        angle = [0, -2 * math.pi / 3, 2 * math.pi / 3]["ABC".index(p)]
        text.append(f"voltage_{p} {literal(240 * np.exp(1j * angle))};")
    text.extend(
        [
            "}",
            f"object load {{ name sink; phases {flags}; nominal_voltage 240; maximum_voltage_error 1e-10;",
        ]
    )
    for p in phases:
        text.append(f"constant_impedance_{p} {literal(loads()[ord(p) - 65])};")
    text.extend(["}", "object underground_line_conductor { name cable;"])
    text.extend(f"{key} {value};" for key, value in properties(kind).items())
    text.extend(["}", "object line_spacing { name spacing;"])
    text.extend(f"distance_{key} {value} ft;" for key, value in distances(kind).items())
    text.extend(["}", "object line_configuration { name code; spacing spacing;"])
    text.extend(f"conductor_{p} cable;" for p in flags)
    text.extend(
        [
            "}",
            f"object underground_line {{ name line; phases {flags}; from source; to sink; length 1 mile; configuration code; }}",
        ]
    )
    return "\n".join(text) + "\n"


def original_snapshot(path, flags, expected):
    raw = read_glm(path, 3, line_class="underground_line")
    result = {k: v for k, v in raw.items() if k in expected}
    phases = flags.replace("N", "")
    meter = recorded(path / "source_meter.csv")
    for p in phases:
        current = meter[f"measured_current_{p}"]
        v = complex(*result[f"source.{p}.voltage"]["value"])
        del result[f"line.from.{p}.current"]
        del result[f"line.from.{p}.power"]
        add(result, f"line.from.{p}.current", "A", current)
        add(result, f"line.from.{p}.power", "VA", v * current.conjugate())
    link = ET.parse(path / "impedance.xml").find("underground_line")

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

    y = 2 * np.linalg.solve(matrix("b"), matrix("a") - np.eye(len(phases)))
    for i, p in enumerate(phases):
        for j, q in enumerate(phases):
            add(result, f"line.y.{p}{q}", "S", y[i, j])
    del result["line.loss"]
    add(
        result,
        "line.loss",
        "VA",
        sum(
            complex(*result[f"line.{side}.{p}.power"]["value"])
            for side in ("from", "to")
            for p in phases
        ),
    )
    return result, raw["line.loss"]


def invalid_profile(args, profile, path, source):
    """Require the reader to reject cases with no valid native circuit."""
    result = dict(
        passed=False, qualification="native-invalid input, not supported physics"
    )
    try:
        command(
            [
                str(args.bridge.resolve()),
                str(source.resolve()),
                str(path.resolve()),
                profile,
            ],
            path,
        )
        result["error"] = "reader accepted a native-invalid geometry"
        return result
    except RuntimeError as error:
        result["reader_error"] = str(error)
        required = "PNNL requires" if profile == "pnnl" else "without N is singular"
        if required not in str(error):
            return result
    dest = path / "native_invalid"
    dest.mkdir()
    text = source.read_text() + instrumentation("ABC")
    (dest / "model.glm").write_text(text)
    try:
        result["version"] = run_glm(profile, dest, args.pnnl_root)
        read_glm(dest, 3, line_class="underground_line")
        result["error"] = "native unexpectedly produced a usable circuit"
    except (RuntimeError, ValueError) as error:
        result["native_error"] = str(error)
        expected = (
            "Cable types specified in configuration are not consistent"
            if profile == "pnnl"
            else "invalid polar value: nan"
        )
        result["passed"] = expected in str(error)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--profile", action="append", choices=["pnnl", "arras"])
    parser.add_argument("--case", action="append")
    parser.add_argument("--solver", choices=["NR", "FBS"], default="NR")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(
        scope=__doc__,
        original_solver=args.solver,
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
    )

    def save(key, result):
        report["cases"][key] = result
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(key, result["passed"], flush=True)

    cases = [
        ("CN606", "ABC", 60),
        ("CN607", "AN", 60),
        ("CN606", "BCN", 60),
        ("CN606", "ABCN", 60),
        ("TS", "AN", 60),
        ("TS", "ABC", 60),
        ("TS", "ABCN", 60),
        ("TS", "BCN", 60),
        ("BARE", "ABCN", 60),
        ("CN607", "C", 60),
        ("CN606", "ABC", 50),
    ]
    for profile in args.profile or ("pnnl", "arras"):
        for kind, flags, f in cases:
            if args.case and f"{kind}/{flags}/{f}" not in args.case:
                continue
            for cap in (False, True):
                key = f"{profile}/{kind}/{flags}/{f}/cap_{cap}"
                path = args.output / key
                path.mkdir(parents=True)
                source = path / "input.glm"
                source.write_text(deck(kind, flags, f, cap, args.solver))
                if (profile == "pnnl" and kind == "BARE") or (
                    profile == "arras" and kind == "TS" and not flags.endswith("N")
                ):
                    result = invalid_profile(args, profile, path, source)
                    save(key + "/native_invalid", result)
                    continue
                try:
                    command(
                        [
                            str(args.bridge.resolve()),
                            str(source.resolve()),
                            str(path.resolve()),
                            profile,
                        ],
                        path,
                    )
                    assert (path / "fresh.glm").read_bytes() == (
                        path / "restored.glm"
                    ).read_bytes()
                except Exception as error:
                    save(key + "/emission", dict(passed=False, error=str(error)))
                    continue
                for variant in ("original", "fresh", "edited"):
                    dest = path / variant
                    dest.mkdir()
                    text = (
                        source if variant == "original" else path / f"{variant}.glm"
                    ).read_text() + instrumentation("ABC")
                    text, extra = instrument(text, flags.replace("N", ""))
                    if variant == "original" and args.solver == "FBS":
                        text += 'object recorder { parent source; property measured_current_A,measured_current_B,measured_current_C; interval 1; file "source_meter.csv"; }\n'
                        extra = (*extra, "source_meter.csv")
                    (dest / "model.glm").write_text(text)
                    result = dict(
                        passed=False,
                        input_sha256=hashlib.sha256(text.encode()).hexdigest(),
                    )
                    try:
                        result["version"] = run_glm(
                            profile,
                            dest,
                            args.pnnl_root,
                            outputs=(
                                "source.csv",
                                "sink.csv",
                                "line.csv",
                                "impedance.xml",
                                *extra,
                            ),
                        )
                        expected = reference(
                            kind,
                            flags,
                            f,
                            cap,
                            edited=variant == "edited",
                            profile=profile,
                        )
                        if variant == "original":
                            actual, loss = original_snapshot(dest, flags, expected)
                        else:
                            actual, loss, witness = physical_snapshot(dest, flags)
                            result["root_meter_witness"] = witness
                            if not witness["passed"]:
                                raise ValueError(
                                    "root meter disagrees with canonical physical ports"
                                )
                        result.update(
                            comparison=compare(expected, actual),
                            native_reported_loss=loss,
                        )
                        negative = compare(
                            reference(
                                kind,
                                flags,
                                f,
                                cap,
                                edited=variant == "edited",
                                omit_return=True,
                                profile=profile,
                            ),
                            actual,
                        )
                        result["geometry_negative_control"] = negative
                        result["negative_control_kind"] = (
                            "conductor resistance +10%"
                            if kind == "BARE"
                            else "omit native return paths"
                        )
                        result["passed"] = (
                            result["comparison"]["passed"] and not negative["passed"]
                        )
                    except Exception as error:
                        result["error"] = str(error)
                    save(key + "/" + variant, result)
    return (
        0
        if report["cases"] and all(c["passed"] for c in report["cases"].values())
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())

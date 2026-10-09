"""Loaded ordinary transformer conversions against independent coil equations."""

import argparse
import cmath
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from oracle import LOCK, add, compare, literal, recorded, run_glm


def parameters(phases, edited=False):
    primary, secondary, rating = (
        (4160.0, 480.0, 500000.0) if len(phases) == 3 else (240.0, 120.0, 10000.0)
    )
    voltage = np.array(
        [
            cmath.rect(
                primary / (3**0.5 if len(phases) == 3 else 1),
                -2 * np.pi * "ABC".index(p) / 3,
            )
            for p in phases
        ]
    )
    loads = np.array([10 + 3j, 20 + 6j, 30 + 5j])[["ABC".index(p) for p in phases]]
    resistance, reactance = 0.011, 0.02
    if edited:
        voltage[0] *= 1.03
        loads /= 1.25
        secondary *= 1.05
        resistance *= 1.2
        reactance *= 0.9
    return (
        voltage,
        loads,
        primary / secondary,
        (resistance + 1j * reactance) * secondary**2 / rating,
    )


def expected(phases, edited=False, wrong_ratio=False):
    voltage, loads, ratio, z = parameters(phases, edited)
    if wrong_ratio:
        ratio *= 1.01
    current = voltage / ratio / (loads + z)
    sink = loads * current
    result = {}
    for p, vf, vt, it in zip(phases, voltage, sink, current, strict=True):
        ip = it / ratio
        for key, unit, value in [
            (f"source.{p}", "V", vf),
            (f"sink.{p}", "V", vt),
            (f"from.{p}.current", "A", ip),
            (f"to.{p}.current", "A", -it),
            (f"from.{p}.power", "VA", vf * ip.conjugate()),
            (f"to.{p}.power", "VA", -vt * it.conjugate()),
        ]:
            add(result, key, unit, value)
    add(result, "loss", "VA", sum(abs(current) ** 2 * z))
    return result


def instrumentation(phases):
    props = ",".join(f"voltage_{p}" for p in phases)
    link = (
        ",".join(
            f"{q}_{end}_{p}"
            for p in phases
            for end in ("in", "out")
            for q in ("current", "power")
        )
        + ",power_losses"
    )
    return f"""\nmodule tape;
object recorder {{ parent source; property {props}; interval 1; file "source.csv"; }}
object recorder {{ parent sink; property {props}; interval 1; file "sink.csv"; }}
object recorder {{ parent x; property {link}; interval 1; file "line.csv"; }}
"""


def header():
    return """#set complex_format=%.15g%+.15gj
#set complex_output_format=RECT
clock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:02'; }
"""


def deck(phases):
    voltage, loads, _, _ = parameters(phases)
    primary, secondary, kva = (4160, 480, 500) if len(phases) == 3 else (240, 120, 10)
    nominal = primary / (3**0.5 if len(phases) == 3 else 1)
    source = " ".join(
        f"voltage_{p} {literal(v)};" for p, v in zip(phases, voltage, strict=True)
    )
    load = " ".join(
        f"constant_impedance_{p} {literal(z)};"
        for p, z in zip(phases, loads, strict=True)
    )
    connection = "WYE_WYE" if len(phases) == 3 else "SINGLE_PHASE"
    return (
        header()
        + f"""module powerflow {{ solver_method NR; }}
object node {{ name source; phases {phases}N; bustype SWING; nominal_voltage {nominal}; {source} }}
object load {{ name sink; phases {phases}N; nominal_voltage {secondary / (3**0.5 if len(phases) == 3 else 1)}; maximum_voltage_error 1e-10; {load} }}
object transformer_configuration {{ name cfg; connect_type {connection}; primary_voltage {primary}; secondary_voltage {secondary}; power_rating {kva}; resistance .011; reactance .02; }}
object transformer {{ name x; phases {phases}N; from source; to sink; configuration cfg; latitude 0; longitude 151; }}
"""
        + instrumentation(phases)
    )


def read_glm(directory, phases):
    source, sink, link = (
        recorded(directory / f"{name}.csv") for name in ("source", "sink", "line")
    )
    result = {}
    for p in phases:
        for bus, values in [("source", source), ("sink", sink)]:
            add(result, f"{bus}.{p}", "V", values[f"voltage_{p}"])
        for side, end, sign in [("from", "in", 1), ("to", "out", -1)]:
            for quantity, unit in [("current", "A"), ("power", "VA")]:
                add(
                    result,
                    f"{side}.{p}.{quantity}",
                    unit,
                    sign * link[f"{quantity}_{end}_{p}"],
                )
    add(result, "loss", "VA", link["power_losses"])
    return result


def read_dss(path, phases):
    import opendssdirect as dss

    dss.Basic.ClearAll()
    dss.Text.Command(f'redirect "{path}"')
    if not dss.Solution.Converged():
        raise ValueError("DSS did not converge")
    if LOCK["dss"]["version"] not in dss.Basic.Version():
        raise ValueError("DSS engine pin mismatch")
    result = {}

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    for name in ("source", "sink"):
        dss.Circuit.SetActiveBus(name)
        values = dict(zip(dss.Bus.Nodes(), unpack(dss.Bus.Voltages()), strict=True))
        for p in phases:
            add(result, f"{name}.{p}", "V", values["ABC".index(p) + 1])
    dss.Circuit.SetActiveElement("transformer.x")
    nodes = dss.CktElement.NodeOrder()
    n = dss.CktElement.NumConductors()
    for q, unit, raw in [
        ("current", "A", dss.CktElement.Currents()),
        ("power", "VA", 1000 * np.array(dss.CktElement.Powers())),
    ]:
        values = unpack(raw)
        for k, side in enumerate(("from", "to")):
            for p in phases:
                index = nodes[k * n : (k + 1) * n].index("ABC".index(p) + 1) + k * n
                add(result, f"{side}.{p}.{q}", unit, values[index])
    add(result, "loss", "VA", complex(*dss.CktElement.Losses()))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", required=True, type=Path)
    parser.add_argument("--pnnl-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "engines": LOCK,
        "cases": [],
    }
    for phases in ("A", "C", "ABC"):
        for profile in ("pnnl", "arras"):
            base = args.output / f"{phases}-{profile}"
            base.mkdir()
            original = base / "input.glm"
            original.write_text(deck(phases))
            converted = base / "converted"
            converted.mkdir()
            run = subprocess.run(
                [str(args.bridge), str(original), str(converted), profile],
                capture_output=True,
                text=True,
                check=True,
            )
            (base / "bridge.json").write_text(run.stdout)
            if (converted / "fresh.glm").read_bytes() != (
                converted / "restored.glm"
            ).read_bytes():
                raise ValueError("IR changed canonical output")
            for variant in ("original", "fresh", "edited"):
                directory = base / variant
                directory.mkdir()
                text = (
                    original.read_text()
                    if variant == "original"
                    else header()
                    + (converted / f"{variant}.glm").read_text()
                    + instrumentation(phases)
                )
                (directory / "model.glm").write_text(text)
                version = run_glm(
                    profile,
                    directory,
                    args.pnnl_root,
                    outputs=("source.csv", "sink.csv", "line.csv"),
                )
                actual = read_glm(directory, phases)
                reference = expected(phases, variant == "edited")
                result = compare(reference, actual)
                result.update(
                    case=f"{phases}/{profile}/{variant}",
                    version=version,
                    negative_detected=not compare(
                        expected(phases, variant == "edited", True), actual
                    )["passed"],
                )
                (directory / "measurements.json").write_text(
                    json.dumps({"expected": reference, "actual": actual}, indent=2)
                )
                report["cases"].append(result)
                print(
                    result["case"], result["passed"], result["failures"][:2], flush=True
                )
            for variant in ("fresh", "edited"):
                actual = read_dss(converted / f"{variant}.dss" / "case.dss", phases)
                reference = expected(phases, variant == "edited")
                (converted / f"{variant}-dss-measurements.json").write_text(
                    json.dumps({"expected": reference, "actual": actual}, indent=2)
                )
                result = compare(reference, actual)
                result.update(
                    case=f"{phases}/{profile}/{variant}/dss",
                    negative_detected=not compare(
                        expected(phases, variant == "edited", True), actual
                    )["passed"],
                )
                report["cases"].append(result)
                print(
                    result["case"], result["passed"], result["failures"][:2], flush=True
                )
            (args.output / "report.json").write_text(json.dumps(report, indent=2))
    return (
        0 if all(r["passed"] and r["negative_detected"] for r in report["cases"]) else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())

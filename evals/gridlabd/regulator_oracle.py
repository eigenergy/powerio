"""Fixed regulator port equations, canonical edits and cross-engine conversion."""

import argparse
import cmath
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from oracle import LOCK, add, compare, literal, recorded, run_glm
from transformer_roundtrip_oracle import header

CASES = {
    "type_a_bank": ("A", "ABC", [10, 8, 11], 0.1, False),
    "type_b_subset": ("B", "BC", [-3, 7], 0.1, False),
    "type_a_default": ("A", "A", [10], 1e-4, False),
    "type_b_override": ("B", "C", [-4], 0.1, True),
}


def expected(case, edited=False, negative=False):
    kind, phases, taps, resistance, _ = CASES[case]
    result = {}
    loss = 0j
    for index, (p, tap) in enumerate(zip(phases, taps, strict=True)):
        voltage = cmath.rect(240, -2 * np.pi * "ABC".index(p) / 3)
        load = [10 + 3j, 20 + 6j, 30 + 5j]["ABC".index(p)]
        amount = tap * 0.1 / 16
        ratio = 1 / (1 + amount) if kind == "A" else 1 - amount
        r = resistance
        if edited:
            if index == 0:
                voltage *= 1.03
            load /= 1.25
            ratio *= 1.02 / 1.05
            r *= 1.2 * 1.05**2
        if negative:
            ratio *= 1.01
        current = voltage / ratio / (load + r)
        sink = load * current
        incoming = current / ratio
        for key, unit, value in [
            (f"source.{p}", "V", voltage),
            (f"sink.{p}", "V", sink),
            (f"from.{p}.current", "A", incoming),
            (f"to.{p}.current", "A", -current),
            (f"from.{p}.power", "VA", voltage * incoming.conjugate()),
            (f"to.{p}.power", "VA", -sink * current.conjugate()),
        ]:
            add(result, key, unit, value)
        loss += abs(current) ** 2 * r
    add(result, "loss", "VA", loss)
    return result


def names(phases, canonical):
    return {
        p: f"x-reg__{p}" if canonical and len(phases) > 1 else "x-reg" for p in phases
    }


def instruments(phases, canonical):
    props = ",".join(f"voltage_{p}" for p in phases)
    text = f"""\nmodule tape;
object recorder {{ parent source; property {props}; interval 1; file "source.csv"; }}
object recorder {{ parent sink; property {props}; interval 1; file "sink.csv"; }}
"""
    outputs = ["source.csv", "sink.csv"]
    for name in dict.fromkeys(names(phases, canonical).values()):
        props = (
            ",".join(
                f"{q}_{end}_{p}"
                for p in phases
                for end in ("in", "out")
                for q in ("current", "power")
            )
            + ",power_losses"
        )
        filename = f"{name}.csv"
        text += f'object recorder {{ parent "{name}"; property {props}; interval 1; file "{filename}"; }}\n'
        outputs.append(filename)
    return text, tuple(outputs)


def deck(case):
    kind, phases, taps, r, override = CASES[case]
    source = " ".join(
        f"voltage_{p} {literal(cmath.rect(240, -2 * np.pi * 'ABC'.index(p) / 3))};"
        for p in phases
    )
    loads = " ".join(
        f"constant_impedance_{p} {literal([10 + 3j, 20 + 6j, 30 + 5j]['ABC'.index(p)])};"
        for p in phases
    )
    tap_props = " ".join(
        f"tap_pos_{p} {9 if override else tap};"
        for p, tap in zip(phases, taps, strict=True)
    )
    objtap = "tap_C -4;" if override else ""
    rtext = "" if r == 1e-4 else f"regulator_resistance {r};"
    return (
        header()
        + f"""module powerflow {{ solver_method NR; }}
object node {{ name source; phases {phases}N; bustype SWING; nominal_voltage 240; {source} }}
object load {{ name sink; phases {phases}N; nominal_voltage 240; maximum_voltage_error 1e-10; {loads} }}
object regulator_configuration {{ name cfg; connect_type WYE_WYE; Type {kind}; Control MANUAL; raise_taps 16; lower_taps 16; regulation .1; {tap_props} }}
object regulator {{ name "x-reg"; phases {phases}N; from source; to sink; configuration cfg; {rtext} {objtap} latitude 0; longitude 151; }}
"""
    )


def read_glm(directory, phases, canonical):
    result = {}
    for bus in ("source", "sink"):
        row = recorded(directory / f"{bus}.csv")
        for p in phases:
            add(result, f"{bus}.{p}", "V", row[f"voltage_{p}"])
    mapping = names(phases, canonical)
    rows = {name: recorded(directory / f"{name}.csv") for name in set(mapping.values())}
    for p, name in mapping.items():
        row = rows[name]
        for side, end, sign in [("from", "in", 1), ("to", "out", -1)]:
            for q, unit in [("current", "A"), ("power", "VA")]:
                add(result, f"{side}.{p}.{q}", unit, sign * row[f"{q}_{end}_{p}"])
    add(result, "loss", "VA", sum(row["power_losses"] for row in rows.values()))
    return result


def read_dss(path, phases):
    import opendssdirect as dss

    dss.Basic.ClearAll()
    dss.Text.Command(f'redirect "{path}"')
    if (
        not dss.Solution.Converged()
        or LOCK["dss"]["version"] not in dss.Basic.Version()
    ):
        raise ValueError("DSS convergence/version failure")

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    result = {}
    for bus in ("source", "sink"):
        dss.Circuit.SetActiveBus(bus)
        v = dict(zip(dss.Bus.Nodes(), unpack(dss.Bus.Voltages()), strict=True))
        for p in phases:
            add(result, f"{bus}.{p}", "V", v["ABC".index(p) + 1])
    loss = 0j
    for p, name in names(phases, True).items():
        dss.Circuit.SetActiveElement("transformer." + name)
        if dss.CktElement.NodeOrder() != ["ABC".index(p) + 1, 0] * 2:
            raise ValueError("unexpected terminal order")
        for q, unit, raw in [
            ("current", "A", dss.CktElement.Currents()),
            ("power", "VA", 1000 * np.array(dss.CktElement.Powers())),
        ]:
            values = unpack(raw)
            for side, index in [("from", 0), ("to", 2)]:
                add(result, f"{side}.{p}.{q}", unit, values[index])
        loss += complex(*dss.CktElement.Losses())
    add(result, "loss", "VA", loss)
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--bridge", type=Path, required=True)
    p.add_argument("--pnnl-root", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": [],
    }
    for case in CASES:
        phases = CASES[case][1]
        for profile in ("pnnl", "arras"):
            base = args.output / f"{case}-{profile}"
            base.mkdir()
            path = base / "input.glm"
            path.write_text(deck(case))
            converted = base / "converted"
            converted.mkdir()
            run = subprocess.run(
                [str(args.bridge), str(path), str(converted), profile],
                capture_output=True,
                text=True,
                check=True,
            )
            (base / "bridge.json").write_text(run.stdout)
            if (converted / "fresh.glm").read_bytes() != (
                converted / "restored.glm"
            ).read_bytes():
                raise ValueError("IR changed output")
            for variant in ("original", "fresh", "edited", "fresh-dss", "edited-dss"):
                edited = variant.startswith("edited")
                directory = base / variant
                directory.mkdir()
                if variant.endswith("-dss"):
                    actual = read_dss(
                        converted / f"{variant[:-4]}.dss" / "case.dss", phases
                    )
                else:
                    instrumentation, outputs = instruments(
                        phases, variant != "original"
                    )
                    text = (
                        path.read_text()
                        if variant == "original"
                        else header() + (converted / f"{variant}.glm").read_text()
                    )
                    (directory / "model.glm").write_text(text + instrumentation)
                    run_glm(profile, directory, args.pnnl_root, outputs=outputs)
                    actual = read_glm(directory, phases, variant != "original")
                reference = expected(case, edited)
                (directory / "measurements.json").write_text(
                    json.dumps({"expected": reference, "actual": actual}, indent=2)
                )
                check = compare(reference, actual)
                check.update(
                    case=f"{case}/{profile}/{variant}",
                    negative_detected=not compare(expected(case, edited, True), actual)[
                        "passed"
                    ],
                )
                report["cases"].append(check)
                print(check["case"], check["passed"], check["failures"][:2], flush=True)
                (args.output / "report.json").write_text(json.dumps(report, indent=2))
    return (
        0 if all(c["passed"] and c["negative_detected"] for c in report["cases"]) else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())

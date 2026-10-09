"""Pinned IEEE13 bus-voltage and source-current conversion qualification.

This is a complete-feeder run with a limited observable scope. Device port
currents, device losses and edited-feeder laws remain separate
qualification work; passing this harness does not prove those requirements.
"""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

from oracle import HERE, LOCK, add, compare, recorded, run_glm
from transformer_roundtrip_oracle import header


def instrument(text, network, original):
    # The hash-pinned input has braces inside line comments. This adaptation
    # is confined to that known fixture, not a general GLM preprocessing API.
    text = re.sub(r"//[^\n]*", "", text)
    # Arras rejects PNNL's voltdump mode POLAR. Our recorders provide the
    # measurements instead; remove only this non-electrical output object.
    text = re.sub(r"object voltdump\s*\{[^{}]*\}", "", text)
    if original:
        text = re.sub(
            r"object (node|load)\s*\{",
            r"object \1 { maximum_voltage_error 1e-9;",
            text,
        )
    text, count = re.subn(
        r'object\s+node\s*\{(?=[^{}]*\bname\s+"?n650[;"\s])',
        "object meter {",
        text,
    )
    if count != 1:
        raise ValueError(f"source meter insertion count {count}")
    text = (
        "#set complex_format=%.15g%+.15gj\n#set complex_output_format=RECT\n"
        + text
        + "\nmodule tape;\n"
    )
    outputs = []
    for i, bus in enumerate(network["buses"]):
        props = ",".join(
            f"voltage_{'ABC'[int(t) - 1]}" for t in bus["terminals"] if t != "n"
        )
        filename = f"bus-{i}.csv"
        outputs.append(filename)
        text += f'object recorder {{ parent "{bus["id"]}"; property {props}; interval 1; file "{filename}"; }}\n'
    text += 'object recorder { parent n650; property measured_current_A,measured_current_B,measured_current_C; interval 1; file "source-meter.csv"; }\n'
    outputs.append("source-meter.csv")
    return text, tuple(outputs)


def snapshot(directory, network):
    result = {}
    for i, bus in enumerate(network["buses"]):
        row = recorded(directory / f"bus-{i}.csv")
        expected = {
            f"voltage_{'ABC'[int(t) - 1]}" for t in bus["terminals"] if t != "n"
        }
        if row.keys() != expected:
            raise ValueError("incomplete native bus voltage inventory")
        for prop, value in row.items():
            add(result, f"{bus['id']}.{prop}", "V", value)
    row = recorded(directory / "source-meter.csv")
    if row.keys() != {f"measured_current_{p}" for p in "ABC"}:
        raise ValueError("incomplete source current inventory")
    for prop, value in row.items():
        add(result, "source." + prop, "A", value)
    return result


def read_dss(path, network):
    import numpy as np
    import opendssdirect as dss

    dss.Basic.ClearAll()
    dss.Text.Command(f'redirect "{path}"')
    if LOCK["dss"]["version"] not in dss.Basic.Version():
        raise ValueError("DSS engine version mismatch")

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    snapshots = {}
    for mode in ("default", "strict"):
        if mode == "strict":
            dss.Text.Command("set tolerance=1e-12 maxiterations=100")
            dss.Solution.Solve()
        if not dss.Solution.Converged() or dss.Solution.ControlMode() != -1:
            raise ValueError("DSS did not converge with controls disabled")
        actual = {}
        for bus in network["buses"]:
            dss.Circuit.SetActiveBus(bus["id"])
            values = dict(zip(dss.Bus.Nodes(), unpack(dss.Bus.Voltages()), strict=True))
            for terminal in bus["terminals"]:
                if terminal != "n":
                    add(
                        actual,
                        f"{bus['id']}.voltage_{'ABC'[int(terminal) - 1]}",
                        "V",
                        values[int(terminal)],
                    )
        # In this pinned fixture the three regulator legs are the only
        # outgoing branches at the source. Read their native primary currents.
        legs = [t for t in network["transformers"] if t["windings"][0]["bus"] == "n650"]
        if len(legs) != 3 or any(t["phases"] != 1 for t in legs):
            raise ValueError("unexpected source topology")
        for transformer in legs:
            phase = int(transformer["windings"][0]["terminal_map"][0])
            name = "transformer." + transformer["name"]
            dss.Circuit.SetActiveElement(name)
            if (
                dss.CktElement.Name().lower() != name.lower()
                or dss.CktElement.NodeOrder() != [phase, 0, phase, 0]
            ):
                raise ValueError("unexpected regulator terminal map")
            add(
                actual,
                f"source.measured_current_{'ABC'[phase - 1]}",
                "A",
                unpack(dss.CktElement.Currents())[0],
            )
        snapshots[mode] = actual
    return snapshots


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--bridge", required=True, type=Path)
    parser.add_argument("--pnnl-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    manifest = json.loads((HERE / "upstream.json").read_text())
    fixture = next(
        f for f in manifest["files"] if f["local"].endswith("test_IEEE_13_NR.glm")
    )
    data = args.source.read_bytes()
    if hashlib.sha256(data).hexdigest() != fixture["sha256"]:
        raise ValueError("IEEE13 source pin mismatch")
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "source": fixture,
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "adaptations": [
            "Strip known input's line comments before instrumenting objects",
            "Omit voltdump; Arras rejects PNNL mode POLAR",
            "Set original bus convergence to canonical 1e-9 V; electrical comparison tolerances unchanged",
            "Replace SWING node with a root meter; preserve source phasors and topology",
            "Retain the original 32 published voltage assertions",
        ],
        "cases": {},
    }
    for profile in ("pnnl", "arras"):
        base = args.output / profile
        base.mkdir()
        converted = base / "converted"
        converted.mkdir()
        run = subprocess.run(
            [str(args.bridge), str(args.source), str(converted), profile],
            capture_output=True,
            text=True,
            check=True,
        )
        (base / "bridge.json").write_text(run.stdout)
        network = json.loads(run.stdout)["fresh"]["network"]
        if (converted / "fresh.glm").read_bytes() != (
            converted / "restored.glm"
        ).read_bytes():
            raise ValueError("IR changed canonical output")
        snapshots = {}
        for label in ("original", "fresh", "negative"):
            directory = base / label
            directory.mkdir()
            text = (
                data.decode()
                if label == "original"
                else header() + (converted / "fresh.glm").read_text()
            )
            if label == "negative":
                text, count = re.subn(
                    r"regulation ([^;]+);", "regulation 0;", text, count=1
                )
                if count != 1:
                    raise ValueError(
                        "negative control did not remove a regulator boost"
                    )
            text, outputs = instrument(text, network, label == "original")
            (directory / "model.glm").write_text(text)
            run_glm(profile, directory, args.pnnl_root, outputs=outputs)
            snapshots[label] = snapshot(directory, network)
        dss_snapshots = read_dss(converted / "fresh.dss" / "case.dss", network)
        snapshots["dss_default"] = dss_snapshots["default"]
        snapshots["dss_strict"] = dss_snapshots["strict"]
        (base / "snapshots.json").write_text(json.dumps(snapshots, indent=2))
        check = compare(snapshots["original"], snapshots["fresh"])
        check["negative_control"] = compare(
            snapshots["original"], snapshots["negative"]
        )
        check["negative_detected"] = not check["negative_control"]["passed"]
        check["dss_default_solve"] = compare(
            snapshots["original"], snapshots["dss_default"]
        )
        check["dss_strict_solve"] = compare(
            snapshots["original"], snapshots["dss_strict"]
        )
        check["dss_solver_settings"] = {
            "tolerance": 1e-12,
            "maxiterations": 100,
            "controls": "off",
        }
        report["cases"][profile] = check
        print(profile, check["passed"], check["max_absolute_error"], flush=True)
        (args.output / "report.json").write_text(json.dumps(report, indent=2))
    return (
        0
        if all(
            c["passed"] and c["negative_detected"] and c["dss_strict_solve"]["passed"]
            for c in report["cases"].values()
        )
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())

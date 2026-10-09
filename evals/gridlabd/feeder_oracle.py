"""Pinned IEEE13/IEEE37 NR static conversion and independently edited cases.

Both GridLAB-D forks and OpenDSS compare all bus phase voltages, source currents,
branch phase-port currents/powers/losses, and original load-object phase currents
and powers: 257 complex quantities for IEEE13, 732 for IEEE37. Every bus must
also satisfy current balance. This qualifies these pinned NR cases and edits,
not arbitrary feeders, explicit neutrals or physical delta transformer isolation.
"""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

import feeder_ports
from oracle import HERE, LOCK, add, compare, literal, parse_complex, recorded, run_glm
from transformer_roundtrip_oracle import header


def instrument(text, network, original):
    source = feeder_ports.source_bus(network)
    # The hash-pinned input has braces inside line comments. This adaptation
    # is confined to that known fixture, not a general GLM preprocessing API.
    text = re.sub(r"//[^\n]*", "", text)
    # A fixed j suffix combined with native POLAR_DEG would mislabel angles
    # as imaginary parts. Output instrumentation must override both settings.
    text = re.sub(r"(?m)^#set complex_output_format=.*(?:\n|$)", "", text)
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
        rf'object\s+(?:node|meter)\s*\{{(?=[^{{}}]*\bname\s+"?{re.escape(source)}[;"\s])',
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
    text += f'object recorder {{ parent "{source}"; property measured_current_A,measured_current_B,measured_current_C; interval 1; file "source-meter.csv"; }}\n'
    outputs.append("source-meter.csv")
    text, ports = feeder_ports.instrument(text, network, original)
    return text, (*outputs, *ports)


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
        ports = feeder_ports.read_dss(dss, network)
        actual.update(ports)
        # Sum native device port currents at the source. No native load is
        # attached directly to either pinned feeder's source bus.
        source = feeder_ports.source_bus(network)
        if any(load["bus"] == source for load in network["loads"]):
            raise ValueError("source load requires additional metering")
        currents = dict.fromkeys("ABC", 0j)
        for name, _, ends in feeder_ports.devices(network):
            for side, (bus, terminals) in zip(("from", "to"), ends, strict=True):
                if bus == source:
                    for terminal in terminals:
                        p = "ABC"[int(terminal) - 1]
                        currents[p] += complex(
                            *ports[f"{name}.{side}.{p}.current"]["value"]
                        )
        for p, current in currents.items():
            add(actual, f"source.measured_current_{p}", "A", current)
        snapshots[mode] = actual
    return snapshots


def edited_original(text, source="n650"):
    """Apply the bridge's documented edits directly to the pinned native case.

    Does not read converted values or writer output. Regulator phases use
    native Type A with independent per-phase settings; the writer uses Type B.
    """
    text = re.sub(r"//[^\n]*", "", text)
    text = re.sub(r"object complex_assert\s*\{[^{}]*\};?", "", text)

    def edit_object(match):
        cls, body = match.groups()
        if cls == "load":

            def scale_load(prop):
                key, law, value = prop.groups()
                factor = 1 / 1.25 if law == "impedance" else 1.25
                return f"{key} {literal(parse_complex(value) * factor)};"

            body = re.sub(
                r"(constant_(power|current|impedance)_[ABC])\s+([^;]+);",
                scale_load,
                body,
            )
        elif cls in ("overhead_line", "underground_line"):
            body, count = re.subn(
                r"length\s+([0-9.]+);",
                lambda m: f"length {float(m[1]) * 1.1:.17g};",
                body,
            )
            if count != 1:
                raise ValueError("unexpected original line length")
        elif cls == "switch":
            body += " switch_resistance 0.00011; "
        elif cls == "transformer_configuration":
            for prop, factor in [
                ("secondary_voltage", 1.05),
                ("resistance", 1.2),
                ("reactance", 0.9),
            ]:
                body, count = re.subn(
                    rf"\b{prop}\s+([0-9.]+);",
                    lambda m, prop=prop, factor=factor: (
                        f"{prop} {float(m[1]) * factor:.17g};"
                    ),
                    body,
                )
                if count != 1:
                    raise ValueError("missing native transformer parameter")
        elif cls in ("node", "meter") and re.search(
            rf'name\s+"?{re.escape(source)}"?\s*;', body
        ):
            body = re.sub(
                r"voltage_A\s+([^;]+);",
                lambda m: f"voltage_A {literal(parse_complex(m[1]) * 1.03)};",
                body,
            )
        elif cls == "regulator_configuration":
            return ""
        elif cls == "regulator":
            result = []
            for phase, tap in zip("ABC", (10, 8, 11), strict=True):
                # Original Type A a=1/(1+tap*.1/16). The typed edit changes
                # primary tap by1.02 and secondary rated volts by1.05.
                ratio = 1 / (1 + tap * 0.1 / 16) * 1.02 / 1.05
                regulation = 1 / ratio - 1
                resistance = 1e-4 * 1.2 * 1.05**2
                name = f"fregn650n630__{phase}"
                result.append(
                    f'object regulator_configuration {{ name "{name}__native_edited"; connect_type WYE_WYE; Control MANUAL; Type A; raise_taps 1; lower_taps 1; regulation {regulation:.17g}; tap_pos_{phase} 1; }}'
                )
                result.append(
                    f'object regulator {{ name "{name}"; phases {phase}N; from n650; to n630; configuration "{name}__native_edited"; regulator_resistance {resistance:.17g}; }}'
                )
            return "\n".join(result)
        return f"object {cls} {{ {body} }}"

    # Native negative switch reactance selects -default_resistance; changing
    # that module default scales the original contact X as well as R.
    text = text.replace(
        "module powerflow {", "module powerflow { default_resistance 0.00011;", 1
    )
    return re.sub(r"object (\w+)\s*\{([^{}]*)\}", edit_object, text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--case", choices=("ieee13", "ieee37"), default="ieee13")
    parser.add_argument("--bridge", required=True, type=Path)
    parser.add_argument("--pnnl-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    manifest = json.loads((HERE / "upstream.json").read_text())
    number = "13" if args.case == "ieee13" else "37"
    fixture = next(
        f
        for f in manifest["files"]
        if f["local"].endswith(f"test_IEEE_{number}_NR.glm")
    )
    data = args.source.read_bytes()
    if hashlib.sha256(data).hexdigest() != fixture["sha256"]:
        raise ValueError("feeder source pin mismatch")
    native_text = data.decode()
    bridge_options = []
    dependency = None
    if args.case == "ieee37":
        dependency = next(
            f
            for f in manifest["files"]
            if f["local"] == "data_configurations_37_node.glm"
        )
        root = args.source.resolve().parent.parent
        config = (root / dependency["local"]).read_bytes()
        if hashlib.sha256(config).hexdigest() != dependency["sha256"]:
            raise ValueError("feeder include pin mismatch")
        native_text = native_text.replace(
            '#include "../data_configurations_37_node.glm";', config.decode()
        )
        bridge_options = ["--acquisition-root", str(root)]
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "source": fixture,
        "dependency": dependency,
        "feeder": args.case,
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "adaptations": [
            "Strip known input's line comments before instrumenting objects",
            "Override source complex_output_format with RECT to match the explicit recorder j suffix",
            "Omit voltdump; Arras rejects PNNL mode POLAR",
            "Set original bus convergence to canonical 1e-9 V; electrical comparison tolerances unchanged",
            "Replace SWING node with a root meter; preserve source phasors and topology",
            f"Retain the original {fixture['assertions']} published voltage assertions; remove them only for deliberately edited inputs",
            "IEEE37 only: flatten the hash-pinned configuration include for native instrumentation; PowerIO reads the original project with an explicit root",
            "Original physical line ports use native a/b impedance dumps; published noncommuting-shunt current reporting is checked separately",
            "Canonical generated shunts and load objects use native coefficients and solved voltages; original parent topology is retained",
            "Independent edits: loads x1.25, line/contact impedance lengths x1.1, winding R x1.2, X x0.9, secondary volts x1.05, regulator primary taps x1.02, source phase A x1.03",
        ],
        "cases": {},
    }
    for profile in ("pnnl", "arras"):
        base = args.output / profile
        base.mkdir()
        converted = base / "converted"
        converted.mkdir()
        run = subprocess.run(
            [
                str(args.bridge),
                str(args.source),
                str(converted),
                profile,
                *bridge_options,
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        (base / "bridge.json").write_text(run.stdout)
        decoded = json.loads(run.stdout)
        network = decoded["fresh"]["network"]
        inventory = {
            key: len(network[key])
            for key in ("buses", "lines", "transformers", "loads")
        }
        expected_inventory = (
            {"buses": 15, "lines": 12, "transformers": 4, "loads": 16}
            if args.case == "ieee13"
            else {"buses": 37, "lines": 35, "transformers": 1, "loads": 32}
        )
        if inventory != expected_inventory:
            raise ValueError("pinned feeder inventory changed")
        if (converted / "fresh.glm").read_bytes() != (
            converted / "restored.glm"
        ).read_bytes():
            raise ValueError("IR changed canonical output")
        snapshots = {}
        balances = {}
        native_reports = {}
        for label in ("original", "fresh", "edited_original", "edited", "negative"):
            directory = base / label
            directory.mkdir()
            original = label in ("original", "edited_original")
            variant = "edited" if label.startswith("edited") else "fresh"
            case_network = decoded[variant]["network"]
            text = (
                native_text
                if original
                else header() + (converted / f"{variant}.glm").read_text()
            )
            if label == "edited_original":
                text = edited_original(text, feeder_ports.source_bus(network))
            if label == "negative":
                if args.case == "ieee13":
                    text, count = re.subn(
                        r"regulation ([^;]+);", "regulation 0;", text, count=1
                    )
                else:
                    text, count = re.subn(
                        r"primary_voltage ([^;]+);",
                        lambda m: f"primary_voltage {float(m[1]) * 1.01:.17g};",
                        text,
                        count=1,
                    )
                if count != 1:
                    raise ValueError(
                        "negative control did not change the winding ratio"
                    )
            text, outputs = instrument(text, case_network, original)
            (directory / "model.glm").write_text(text)
            run_glm(profile, directory, args.pnnl_root, outputs=outputs)
            snapshots[label] = snapshot(directory, case_network)
            ports, raw, native_report = feeder_ports.snapshot(
                directory, case_network, original
            )
            native_reports[label] = native_report
            snapshots[label].update(ports)
            balances[label] = feeder_ports.kcl(snapshots[label], case_network)
            (directory / "raw-ports.json").write_text(json.dumps(raw, indent=2))
        dss_snapshots = read_dss(converted / "fresh.dss" / "case.dss", network)
        snapshots["dss_default"] = dss_snapshots["default"]
        snapshots["dss_strict"] = dss_snapshots["strict"]
        edited_dss = read_dss(
            converted / "edited.dss" / "case.dss", decoded["edited"]["network"]
        )
        snapshots["edited_dss_default"] = edited_dss["default"]
        snapshots["edited_dss_strict"] = edited_dss["strict"]
        balances["edited_dss_strict"] = feeder_ports.kcl(
            edited_dss["strict"], decoded["edited"]["network"]
        )
        (base / "snapshots.json").write_text(json.dumps(snapshots, indent=2))
        balances["dss_strict"] = feeder_ports.kcl(snapshots["dss_strict"], network)
        check = compare(snapshots["original"], snapshots["fresh"])
        check["kcl"] = balances
        check["inventory"] = inventory
        check["native_reporting"] = native_reports
        check["edited_glm"] = compare(snapshots["edited_original"], snapshots["edited"])
        check["edited_dss"] = compare(
            snapshots["edited_original"], snapshots["edited_dss_strict"]
        )
        check["stale_output_detected"] = not compare(
            snapshots["edited_original"], snapshots["fresh"]
        )["passed"]
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
        check["accepted"] = (
            check["passed"]
            and check["negative_detected"]
            and check["dss_strict_solve"]["passed"]
            and check["edited_glm"]["passed"]
            and check["edited_dss"]["passed"]
            and check["stale_output_detected"]
            and all(v["passed"] for v in balances.values())
            and all(
                v["reported_current_law"]["passed"] for v in native_reports.values()
            )
        )
        report["cases"][profile] = check
        print(
            profile,
            "accepted",
            check["accepted"],
            "observables",
            check["observables"],
            flush=True,
        )
        (args.output / "report.json").write_text(json.dumps(report, indent=2))
    return 0 if all(c["accepted"] for c in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())

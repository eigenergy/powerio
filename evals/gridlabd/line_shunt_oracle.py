"""Qualify unequal, lossy and reversed line-end shunts with independent KCL.

The validation bridge injects typed SI shunt matrices before IR serialization.
Each fresh/IR/edited/rebased/reference output is solved in both pinned forks.
FBS meters measure generated loads; NR uses native coefficient readback plus a
root-meter current witness. No expected shunt currents are inserted into
observed port quantities. Endpoints are normalized by electrical identity.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

import numpy as np
from oracle import add, command, compare, recorded, run_glm
from pair_load_oracle import deck, parameters
from roundtrip_oracle import instrumentation
from shunt_measurement import instrument, measurements


def shunts(case):
    g = np.array([[2, -0.4, -0.2], [-0.4, 3, -0.3], [-0.2, -0.3, 4]]) * 1e-4
    b = np.array([[6, -2, 0.3], [-2, -7, -0.4], [0.3, -0.4, 8]]) * 1e-4
    source = g + 1j * b
    sink = 0.5 * g - 1.5j * b
    if case == "source_only":
        sink *= 0
    if case == "sink_only":
        source *= 0
    return source, sink


def reference(case, edited=False, defect=None):
    source, z, loads = parameters()
    ys, yt = shunts(case)
    if edited:
        source[0] *= 1.03
        z *= 1.1
        loads /= 1.25
        ys *= 1.1
        yt *= 1.1
    if defect == "omit":
        ys *= 0
        yt *= 0
    elif defect == "swap":
        ys, yt = yt, ys
    series = np.linalg.inv(z)
    sink = np.linalg.solve(series + yt + np.diag(1 / loads), series @ source)
    current = series @ (source - sink)
    from_current = current + ys @ source
    to_current = -current + yt @ sink
    residual = float(max(abs(to_current + sink / loads)))
    if residual > 1e-9:
        raise ValueError(f"independent end-shunt KCL residual {residual}")
    result = {}
    for bus, v in (("source", source), ("sink", sink)):
        for p, value in zip("ABC", v, strict=True):
            add(result, f"{bus}.{p}", "V", value)
    for side, v, i in (("from", source, from_current), ("to", sink, to_current)):
        for p, voltage, amps in zip("ABC", v, i, strict=True):
            add(result, f"line.{side}.{p}.current", "A", amps)
            add(result, f"line.{side}.{p}.power", "VA", voltage * amps.conjugate())
    add(
        result,
        "line.loss",
        "VA",
        sum(source * from_current.conjugate() + sink * to_current.conjugate()),
    )
    return result, residual


def snapshot(directory, include_shunts=True):
    text = (directory / "model.glm").read_text()
    ends = re.search(
        r'object overhead_line\s*\{[^}]*?from "([^"]+)"; to "([^"]+)";', text
    )
    if not ends or "source" not in ends.groups():
        raise ValueError("unexpected canonical line endpoints")
    reverse = ends[1] != "source"
    line = recorded(directory / "line.csv")
    currents = [
        np.array([line[f"current_in_{p}"] for p in "ABC"]),
        -np.array([line[f"current_out_{p}"] for p in "ABC"]),
    ]
    powers = [
        np.array([line[f"power_in_{p}"] for p in "ABC"]),
        -np.array([line[f"power_out_{p}"] for p in "ABC"]),
    ]
    if reverse:
        currents.reverse()
        powers.reverse()
    loss = line["power_losses"]
    if include_shunts:
        for _, bus, v, i in measurements(directory, "ABC"):
            side = 0 if bus == "source" else 1
            if bus not in ("source", "middle", "sink"):
                raise ValueError("unrecognized shunt port")
            power = np.array(v) * np.array(i).conjugate()
            currents[side] += i
            powers[side] += power
            loss += sum(power)
    result = {}
    for bus in ("source", "sink"):
        row = recorded(directory / (bus + ".csv"))
        for p in "ABC":
            add(result, f"{bus}.{p}", "V", row["voltage_" + p])
    for index, side in enumerate(("from", "to")):
        for i, p in enumerate("ABC"):
            add(result, f"line.{side}.{p}.current", "A", currents[index][i])
            add(result, f"line.{side}.{p}.power", "VA", powers[index][i])
    add(result, "line.loss", "VA", loss)
    return result


def source_current_witness(path, expected):
    meter = recorded(path / "source_meter.csv")
    line = recorded(path / "line.csv")
    reverse = not re.search(
        r'object overhead_line\s*\{[^}]*?from "source";',
        (path / "model.glm").read_text(),
    )
    witness = {}
    wanted = {}
    for p in "ABC":
        key = f"line.from.{p}.current"
        wanted[key] = expected[key]
        # NR meter current_inj includes local loads and outgoing links, not
        # incoming links. Normalize a reversed series link at the source port.
        amps = meter[f"measured_current_{p}"]
        if reverse:
            amps -= line[f"current_out_{p}"]
        add(witness, key, "A", amps)
    return compare(wanted, witness)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    for profile in ("pnnl", "arras"):
        for solver in ("NR", "FBS"):
            for case in ("source_only", "sink_only", "unequal_lossy", "reversed_lossy"):
                key = f"{profile}/{solver}/{case}"
                directory = args.output / key
                directory.mkdir(parents=True)
                text = deck([], solver)
                # The source deck's series line is one mile; typed linecode
                # admittances are per metre, while the reference uses total Y.
                ys, yt = (y / 1609.344 for y in shunts(case))
                if case == "reversed_lossy":
                    end = "middle" if solver == "FBS" else "sink"
                    text = text.replace(
                        f"from source; to {end};", f"from {end}; to source;"
                    )
                    ys, yt = yt, ys
                source = directory / "input.glm"
                source.write_text(text)
                override = directory / "shunts.json"
                override.write_text(
                    json.dumps(
                        {
                            "g_from": ys.real.tolist(),
                            "b_from": ys.imag.tolist(),
                            "g_to": yt.real.tolist(),
                            "b_to": yt.imag.tolist(),
                        }
                    )
                )
                decoded = json.loads(
                    command(
                        [
                            str(args.bridge.resolve()),
                            str(source.resolve()),
                            str(directory.resolve()),
                            profile,
                            str(override.resolve()),
                        ],
                        directory,
                    )
                )
                if (directory / "fresh.glm").read_bytes() != (
                    directory / "restored.glm"
                ).read_bytes():
                    raise ValueError("IR changed shunt equivalent output")
                for variant in (
                    "fresh",
                    "restored",
                    "edited",
                    "rebased",
                    "grounded_reference",
                ):
                    path = directory / variant
                    path.mkdir()
                    canonical = (directory / (variant + ".glm")).read_text()
                    model, extra = instrument(canonical + instrumentation("ABC"), "ABC")
                    (path / "model.glm").write_text(model)
                    item = {
                        "passed": False,
                        "canonical_sha256": hashlib.sha256(
                            canonical.encode()
                        ).hexdigest(),
                        "input_sha256": hashlib.sha256(model.encode()).hexdigest(),
                        "diagnostics": decoded[variant]["diagnostics"],
                    }
                    try:
                        item["version"] = run_glm(
                            profile,
                            path,
                            args.pnnl_root,
                            ("source.csv", "sink.csv", "line.csv", "impedance.xml")
                            + extra,
                        )
                        actual = snapshot(path)
                        expected, residual = reference(case, variant == "edited")
                        item["kcl_residual_amperes"] = residual
                        item["comparison"] = compare(expected, actual)
                        if solver == "NR":
                            item["native_source_current_witness"] = (
                                source_current_witness(path, expected)
                            )
                        item["negative_controls"] = {
                            defect: compare(
                                reference(case, variant == "edited", defect)[0], actual
                            )
                            for defect in ("omit", "swap")
                        }
                        item["negative_controls"]["bare_series_port"] = compare(
                            expected, snapshot(path, include_shunts=False)
                        )
                        item["passed"] = item["comparison"]["passed"] and all(
                            not n["passed"] for n in item["negative_controls"].values()
                        )
                        if solver == "NR":
                            item["passed"] &= item["native_source_current_witness"][
                                "passed"
                            ]
                        if variant == "fresh":
                            plain = directory / "without_shunt_meters"
                            plain.mkdir()
                            plain_text = canonical + instrumentation("ABC")
                            (plain / "model.glm").write_text(plain_text)
                            item["uninstrumented_version"] = run_glm(
                                profile, plain, args.pnnl_root
                            )
                            item["uninstrumented_input_sha256"] = hashlib.sha256(
                                plain_text.encode()
                            ).hexdigest()
                            item["instrumentation_equivalence"] = compare(
                                snapshot(plain, include_shunts=False),
                                snapshot(path, include_shunts=False),
                            )
                            item["passed"] &= item["instrumentation_equivalence"][
                                "passed"
                            ]
                    except Exception as error:
                        item["error"] = str(error)
                    report["cases"][key + "/" + variant] = item
                    (args.output / "report.json").write_text(
                        json.dumps(report, indent=2) + "\n"
                    )
                    print(
                        key, variant, "passed" if item["passed"] else item, flush=True
                    )
    return 0 if all(v["passed"] for v in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())

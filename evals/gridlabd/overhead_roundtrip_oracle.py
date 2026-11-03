"""Qualify materialized geometry through fresh, edited and IR-restored GLM.

Physical ports aggregate observed series-link and generated end-shunt currents.
Published native power_losses remains recorded separately because it clamps
individual real phase losses. NR root meters independently witness source
current; native shunt readback supplies coefficients, never expected values.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from oracle import add, command, compare, read_glm, recorded, run_glm
from overhead_geometry_oracle import deck, reference, snapshot
from roundtrip_oracle import instrumentation
from shunt_measurement import PAIRS, instrument, loads, measurements


def physical_snapshot(path, flags):
    phases = flags.replace("N", "")
    actual = snapshot(path, flags)
    native = read_glm(path, 3)
    raw_loss = native["line.loss"]
    for _, bus, voltage, current in measurements(path, phases):
        side = "from" if bus == "source" else "to" if bus == "sink" else None
        if side is None:
            raise ValueError("unknown generated shunt endpoint")
        for p, v, amps in zip(phases, voltage, current, strict=True):
            for quantity, value in [("current", amps), ("power", v * amps.conjugate())]:
                key = f"line.{side}.{p}.{quantity}"
                before = complex(*actual[key]["value"])
                actual[key]["value"] = [
                    float((before + value).real),
                    float((before + value).imag),
                ]
    y = np.zeros((len(phases), len(phases)), complex)
    for name, _ in loads((path / "model.glm").read_text()):
        row = recorded(path / (name + ".csv"))
        for pair in PAIRS:
            z = row[f"constant_impedance_{pair}"]
            if z == 0:
                continue
            m = np.zeros(len(phases))
            m[phases.index(pair[0])] = 1
            if pair[1] != "N":
                m[phases.index(pair[1])] = -1
            y += np.outer(m, m) / z
    for i, p in enumerate(phases):
        for j, q in enumerate(phases):
            key = f"line.y.{p}{q}"
            value = complex(*actual[key]["value"]) + y[i, j]
            actual[key]["value"] = [float(value.real), float(value.imag)]
    loss = sum(
        complex(*actual[f"line.{side}.{p}.power"]["value"])
        for side in ("from", "to")
        for p in phases
    )
    actual["line.loss"]["value"] = [float(loss.real), float(loss.imag)]
    meter = recorded(path / "source_meter.csv")
    witness, expected = {}, {}
    for p in phases:
        add(witness, p, "A", meter[f"measured_current_{p}"])
        add(expected, p, "A", complex(*actual[f"line.from.{p}.current"]["value"]))
    return actual, raw_loss, compare(expected, witness)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--case", action="append")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    cases = [(flags, 60) for flags in ("A", "AN", "BC", "BCN", "ABC", "ABCN")] + [
        ("ABCN", 50)
    ]

    def save(key, result):
        report["cases"][key] = result
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(key, result["passed"], flush=True)

    for profile in ("pnnl", "arras"):
        for flags, f in cases:
            if args.case and f"{flags}/{f}" not in args.case:
                continue
            for cap in (False, True):
                key = f"{profile}/{flags}/{f}/cap_{cap}"
                path = args.output / key
                path.mkdir(parents=True)
                source = path / "input.glm"
                source.write_text(deck(flags, f, cap, "NR"))
                try:
                    decoded = json.loads(
                        command(
                            [
                                str(args.bridge.resolve()),
                                str(source.resolve()),
                                str(path.resolve()),
                                profile,
                            ],
                            path,
                        )
                    )
                    assert (path / "fresh.glm").read_bytes() == (
                        path / "restored.glm"
                    ).read_bytes()
                except Exception as error:
                    save(key + "/emission", {"passed": False, "error": str(error)})
                    continue
                for variant in (
                    "fresh",
                    "restored",
                    "edited",
                    "rebased",
                    "grounded_reference",
                ):
                    dest = path / variant
                    dest.mkdir()
                    text = (path / (variant + ".glm")).read_text() + instrumentation(
                        "ABC"
                    )
                    text, extra = instrument(text, flags.replace("N", ""))
                    (dest / "model.glm").write_text(text)
                    result = {
                        "passed": False,
                        "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
                        "diagnostics": decoded[variant]["diagnostics"],
                    }
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
                        actual, raw_loss, witness = physical_snapshot(dest, flags)
                        expected, residual = reference(
                            flags, f, cap, edited=variant == "edited"
                        )
                        result.update(
                            comparison=compare(expected, actual),
                            published_series_loss=raw_loss,
                            source_meter_witness=witness,
                            kcl_residual_A=residual,
                        )
                        result["passed"] = (
                            result["comparison"]["passed"] and witness["passed"]
                        )
                        if flags.endswith("N"):
                            result["neutral_omission_negative_control"] = compare(
                                reference(
                                    flags,
                                    f,
                                    cap,
                                    omit_neutral=True,
                                    edited=variant == "edited",
                                )[0],
                                actual,
                            )
                            result["passed"] &= not result[
                                "neutral_omission_negative_control"
                            ]["passed"]
                    except Exception as error:
                        result["error"] = str(error)
                    save(key + "/" + variant, result)
    return 0 if all(v["passed"] for v in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())

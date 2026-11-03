"""Establish the pinned IEEE8500 static native reference before PowerIO conversion.

This is cross-fork native evidence at the dumps' six-decimal precision, not
PowerIO conversion qualification. Full-precision ports and edited circuits
remain separate acceptance requirements. No dataset contents are vendored.
"""

import argparse
import csv
import hashlib
import json
import math
import re
from pathlib import Path

from oracle import LOCK, add, compare, run_glm

SOURCE_SHA256 = "995abd0300b4f6c344a4bf07fba3ffe7ef6635fd7cfa8e4316b5fc0bdc9a2791"
OUTPUTS = (
    "Voltage_8500.csv",
    "Current_8500.csv",
    "capacitor_state.csv",
    "regulator_taps.csv",
)


def prepare(data, profile, static):
    if hashlib.sha256(data).hexdigest() != SOURCE_SHA256:
        raise ValueError("IEEE8500 source pin mismatch")
    text = data.decode().replace(
        "stoptime '2000-09-01 12:01:00'", "stoptime '2000-09-01 12:00:01'"
    )
    text, count = re.subn(r"\binterval 60;", "interval 1;", text)
    if count != 3:
        raise ValueError("recorder inventory changed")
    # Both original dump declarations are invalid on at least one pinned fork.
    text = text.replace("mode polar;", "mode RECT;")
    if profile == "arras":
        text = re.sub(r"(object voltdump\s*\{[^{}]*)mode RECT;", r"\1mode rect;", text)
    if text.count("CapBank2:switchC;") != 1:
        raise ValueError("capacitor recorder inventory changed")
    text = text.replace(
        "CapBank2:switchC;",
        "CapBank2:switchC,CapBank3:switchA,CapBank3:switchB,CapBank3:switchC;",
    )
    if static:
        text, caps = re.subn(r"\bcontrol\s+VARVOLT;", "control MANUAL;", text)
        if caps != 3:
            raise ValueError("controlled capacitor inventory changed")
        loads = 0

        def convert(match):
            nonlocal loads
            cls, body = match.groups()
            if cls == "triplex_node" and re.search(r"\bpower_[12]\s", body):
                cls = "triplex_load"
                body = re.sub(r"\bpower_([12])\s", r"constant_power_\1 ", body)
                loads += 1
            return f"object {cls} {{{body}}}"

        # Bounded to this exact pinned input. No general GLM rewriting API.
        text = re.sub(r"object (\w+)\s*\{([^{}]*)\}", convert, text)
        if loads != 1177:
            raise ValueError("triplex load inventory changed")
    return text


def snapshot(directory):
    result = {}
    for filename, unit, prefix, count in [
        ("Voltage_8500.csv", "V", "volt", 4879),
        ("Current_8500.csv", "A", "curr", 4876),
    ]:
        rows = list(
            csv.DictReader(
                line
                for line in (directory / filename).read_text().splitlines()
                if not line.startswith("#")
            )
        )
        if len(rows) != count:
            raise ValueError("native dump inventory changed")
        for row in rows:
            name = row.pop("node_name" if unit == "V" else "link_name")
            row.pop("datetime", None)
            expected = {
                f"{prefix}{p}_{part}" for p in "ABC" for part in ("real", "imag")
            }
            if set(row) != expected:
                raise ValueError("native dump columns changed")
            for phase in "ABC":
                value = complex(
                    float(row[f"{prefix}{phase}_real"]),
                    float(row[f"{prefix}{phase}_imag"]),
                )
                if not math.isfinite(abs(value)):
                    raise ValueError("nonfinite native dump")
                add(result, f"{unit}.{name}.{phase}", unit, value)
    return result


def states(directory):
    result = {}
    for filename in OUTPUTS[2:]:
        lines = (directory / filename).read_text().splitlines()
        header = next(
            line.removeprefix("# ") for line in lines if line.startswith("# timestamp,")
        )
        rows = list(
            csv.DictReader(
                [header] + [line for line in lines if line and not line.startswith("#")]
            )
        )
        if not rows:
            raise ValueError("empty fixed-state recorder")
        result[filename] = [
            {k: v for k, v in row.items() if k != "timestamp"} for row in rows
        ]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    data = args.source.read_bytes()
    report = {
        "scope": __doc__,
        "source_sha256": SOURCE_SHA256,
        "engines": LOCK,
        "adaptations": [
            "Stop one second after original start; recorder intervals one second",
            "Use pinned fork-specific rectangular dump mode spellings",
            "Static variants freeze three VARVOLT capacitors at declared states; fourth bank and regulator taps are already manual",
            "Static variants translate 1177 triplex_node power_1/power_2 declarations to triplex_load constant_power_1/constant_power_2",
            "No solver, impedance, load demand, source voltage or tap changes",
        ],
        "cases": {},
    }
    measured = {}
    for label, profile, static in [
        ("original", "pnnl", False),
        ("pnnl_static", "pnnl", True),
        ("arras_static", "arras", True),
    ]:
        directory = args.output / label
        directory.mkdir()
        text = prepare(data, profile, static)
        (directory / "model.glm").write_text(text)
        run_glm(profile, directory, args.pnnl_root, outputs=OUTPUTS)
        measured[label] = snapshot(directory)
        (directory / "measurements.json").write_text(
            json.dumps(measured[label], indent=2) + "\n"
        )
        report["cases"][label] = {
            "states": states(directory),
            "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
        }
        print(label, len(measured[label]), "complex dump values", flush=True)
    report["comparisons"] = {
        label: compare(measured["original"], measured[label])
        for label in ("pnnl_static", "arras_static")
    }
    report["fixed_states_match"] = all(
        report["cases"][label]["states"] == report["cases"]["original"]["states"]
        for label in ("pnnl_static", "arras_static")
    )
    report["passed"] = report["fixed_states_match"] and all(
        c["passed"] for c in report["comparisons"].values()
    )
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

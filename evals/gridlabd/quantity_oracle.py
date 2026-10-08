"""Compare interpreted GLM quantities with pinned engines and analytic physics.

This validates source constants and unit conversion, not a canonical network
reader/writer. Unused equipment properties expose native conversion independently
of solver initialization. The energized circuit checks the physical consequence
of voltage, impedance and length unit conversion.
"""

import argparse
import hashlib
import json
from pathlib import Path

from oracle import analytic, cases, command, compare, glm, read_glm, recorded, run_glm


def deck(profile: str) -> str:
    text = (
        glm(*cases()["single_phase"])
        .replace("%.15g%+.15gj", "%.15g%+.15g%c")
        .replace("complex_output_format=RECT", "complex_output_format=DEFAULT")
    )
    replacements = {
        "nominal_voltage 240;": "nominal_voltage .24 kV;",
        "voltage_A 240+0j;": "voltage_A .24+0d kV;",
        "constant_impedance_A 20+5j;": "constant_impedance_A .020+.005i kOhm;",
        "z11 1+0.5j;": "z11 .001+.0005j kOhm/mile;",
        "length 5280 ft;": "length 1.609344 km;",
    }
    for old, new in replacements.items():
        if old not in text:
            raise ValueError(f"oracle template changed: {old}")
        text = text.replace(old, new)
    parenthesized = '"(3+4j)"' if profile == "arras" else "3+4j"
    text += f"""
object line_configuration {{ name probe;
    z11 "5 kOhm/mile";
    z12 3+4j mOhm/km;
    z13 5+53.13010235415598d;
    z21 5+0.9272952180016122r;
    z22 3e-3-4e-3i kOhm/mile;
    z23 {parenthesized};
    z31 "5j";
    z32 5 kOhm/mile;
    z33 "";
    c11 1 uF/km;
    rating.summer.continuous 1 kA;
}}
object recorder {{ parent probe;
    property z11,z12,z13,z21,z22,z23,z31,z32,z33,c11,rating.summer.continuous;
    interval 1; file "quantities.csv";
}}
"""
    return "#set double_format=%.15g\n" + text


def reference() -> dict:
    return {
        "source.nominal_voltage": 240,
        "source.voltage_A": 240,
        "sink.nominal_voltage": 240,
        "sink.constant_impedance_A": 20 + 5j,
        "cfg.z11": 1 + 0.5j,
        "line.length": 5280,
        "probe.z11": 5,
        "probe.z12": (3 + 4j) * 0.001 * 1.609344,
        "probe.z13": 3 + 4j,
        "probe.z21": 3 + 4j,
        "probe.z22": 3 - 4j,
        "probe.z23": 3 + 4j,
        "probe.z31": 5,
        "probe.z32": 5000,
        "probe.z33": 0,
        "probe.c11": 1609.344,
        "probe.rating.summer.continuous": 1000,
    }


def check_values(actual: dict, expected: dict, tolerance: float) -> None:
    if actual.keys() != expected.keys():
        raise ValueError("quantity inventory mismatch")
    for key, wanted in expected.items():
        if not abs(actual[key] - wanted) <= tolerance * max(1, abs(wanted)):
            raise ValueError(f"quantity mismatch: {key}: {actual[key]} != {wanted}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--reader", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    for profile in ("pnnl", "arras"):
        directory = args.output.resolve() / profile
        directory.mkdir()
        text = deck(profile)
        source = directory / "model.glm"
        source.write_text(text)
        parsed = json.loads(
            command([str(args.reader.resolve()), str(source), profile], directory)
        )
        check_values(
            {key: complex(*entry["value"]) for key, entry in parsed.items()},
            reference(),
            1e-12,
        )
        warnings = {
            key: entry["diagnostics"]
            for key, entry in parsed.items()
            if entry["diagnostics"]
        }
        if warnings != {
            "probe.z11": ["READ.GLM.UNIT_IGNORED"],
            "probe.z31": ["READ.GLM.UNIT_IGNORED"],
        }:
            raise ValueError("missing or unexpected native ignored-unit diagnostic")
        version = run_glm(
            profile,
            directory,
            args.pnnl_root.resolve(),
            ("source.csv", "sink.csv", "line.csv", "impedance.xml", "quantities.csv"),
        )
        native = {
            "probe." + key: value
            for key, value in recorded(directory / "quantities.csv").items()
        }
        check_values(
            native,
            {
                key: value
                for key, value in reference().items()
                if key.startswith("probe.")
            },
            1e-12,
        )
        electrical = compare(analytic(*cases()["single_phase"]), read_glm(directory, 1))
        if not electrical["passed"]:
            raise ValueError(f"{profile} unit-converted circuit differs electrically")
        report["cases"][profile] = {
            "engine": version,
            "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
            "source_quantities": parsed,
            "native_quantities": {key: [v.real, v.imag] for key, v in native.items()},
            "electrical": electrical,
            "passed": True,
        }
        print(
            profile,
            "PASS",
            len(parsed),
            "source quantities, native properties and electrical reference",
        )
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

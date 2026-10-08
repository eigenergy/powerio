"""Calibrate declared versus initialized object coordinates in pinned engines.

These synthetic cases qualify engine behavior, not PowerIO coordinate conversion.
Native saved GLM rounds coordinates to hundredths of an arcsecond. Tape recorders
cannot read these common object-header fields in the pinned PNNL engine.
"""

import argparse
import hashlib
import json
import math
import re
from pathlib import Path

from oracle import cases, glm, run_glm


def deck() -> str:
    source = glm(*cases()["single_phase"])
    source = source.replace(
        "name source;", "name source; latitude -27.47; longitude 153.02;"
    ).replace("name sink;", "name sink; latitude 27S28:12; longitude 153E1:12;")
    # Deliberately differs from the midpoint: powerflow link initialization
    # overwrites the common header's declared point in the pinned engines.
    source = source.replace("name line;", "name line; latitude 10; longitude 20;")
    source += """
object line_configuration { name zero; latitude 0; longitude 0; }
object line_configuration { name minutes; latitude 27S28.2; longitude 153E1.2; }
object line_configuration { name northern; latitude 45N30; longitude 120W15; }
object line_configuration { name absent; }
object line_configuration { name partial; latitude 0; }
object line_configuration {
    name reference;
    latitude (source.latitude);
    longitude (source.longitude);
}
"""
    return "#set savefile=coordinates.glm\n" + source


def coordinate(text: str, axis: str) -> float:
    """Decode the engine's saved DMS spelling, independently of its input text."""
    if text.startswith('"'):
        text = json.loads(text)
    pattern = r"(\d+)([NS])(\d+):([\d.]+)" if axis == "latitude" else r"(\d+)([EW])(\d+):([\d.]+)"
    match = re.fullmatch(pattern, text)
    if match is None:
        raise ValueError(f"unexpected saved {axis}: {text!r}")
    degree, hemisphere, minute, second = match.groups()
    value = int(degree) + int(minute) / 60 + float(second) / 3600
    return -value if hemisphere in "SW" else value


def locations(path: Path, names: set[str]) -> dict:
    result = {}
    for body in re.findall(r"^object [\w.:]+ \{(.*?)^\}", path.read_text(), re.M | re.S):
        match = re.search(r"^\s*name ([^;]+);", body, re.M)
        if match is None:
            continue
        name = json.loads(match[1]) if match[1].startswith('"') else match[1]
        if name not in names:
            continue
        if name in result:
            raise ValueError(f"duplicate saved object: {name}")
        point = {}
        for axis in ("latitude", "longitude"):
            values = re.findall(rf"^\s*{axis} ([^;]+);", body, re.M)
            if len(values) > 1:
                raise ValueError(f"duplicate {axis} for {name}")
            if values:
                point[axis] = coordinate(values[0], axis)
        result[name] = point
    if set(result) != names:
        raise ValueError(f"missing saved objects: {names - result.keys()}")
    return result


def expected() -> dict:
    south = {"latitude": -27.47, "longitude": 153.02}
    return {
        "source": south,
        "sink": south,
        "line": south,
        "zero": {"latitude": 0, "longitude": 0},
        "minutes": south,
        "northern": {"latitude": 45.5, "longitude": -120.25},
        "absent": {},
        "partial": {"latitude": 0},
        "reference": south,
    }


def check(actual: dict) -> None:
    reference = expected()
    if actual.keys() != reference.keys():
        raise ValueError("coordinate object inventory mismatch")
    for name, point in reference.items():
        if point.keys() != actual[name].keys():
            raise ValueError(f"coordinate absence mismatch: {name}")
        for axis, value in point.items():
            # Half a hundredth of an arcsecond, plus floating-point allowance.
            if not math.isclose(value, actual[name][axis], rel_tol=0, abs_tol=1.4e-6):
                raise ValueError(f"coordinate mismatch: {name}.{axis}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    source = deck()
    report = {
        "scope": __doc__,
        "input_sha256": hashlib.sha256(source.encode()).hexdigest(),
        "declared_line": {"latitude": 10, "longitude": 20},
        "initialized_line_policy": "midpoint of endpoints; replaces declared location",
        "cases": {},
    }
    for profile in ("pnnl", "arras"):
        directory = args.output.resolve() / profile
        directory.mkdir()
        (directory / "model.glm").write_text(source)
        version = run_glm(profile, directory, args.pnnl_root.resolve(), ("coordinates.glm",))
        actual = locations(directory / "coordinates.glm", set(expected()))
        check(actual)
        report["cases"][profile] = {"engine": version, "locations": actual, "passed": True}
        print(profile, "PASS", len(actual), "object coordinate states")
    (args.output / "report.json").write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

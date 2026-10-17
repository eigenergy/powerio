"""Explicit four-wire neutral/earth oracle with finite grounding impedances.

This calibrates OpenDSS extraction. It does not assert that either GridLAB-D
fork implements the same unreduced neutral model. Source and sink neutral
potentials are unknowns; neither is silently tied to earth or Kron reduced.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from oracle import LOCK, add, cases, compare, dss_deck, read_dss

Z_LINE = np.array(
    [
        [1 + 0.5j, 0.1 + 0.2j, 0.07 + 0.15j, 0.1 + 0.12j],
        [0.1 + 0.2j, 0.8 + 0.6j, 0.09 + 0.18j, 0.08 + 0.1j],
        [0.07 + 0.15j, 0.09 + 0.18j, 1.2 + 0.4j, 0.09 + 0.11j],
        [0.1 + 0.12j, 0.08 + 0.1j, 0.09 + 0.11j, 0.7 + 0.3j],
    ]
)
Z_SOURCE_GROUND = 0.2 + 0.05j
Z_SINK_GROUND = 2 + 0.4j


def deck(sink_ground: bool) -> str:
    source, _, loads = cases()["coupled_unbalanced"]
    # Reuse the ordinary model's source/load definitions, then explicitly
    # replace the line and attach each load's return to conductor 4.
    lines = dss_deck(source, Z_LINE[:3, :3], loads).splitlines()
    lines = [s for s in lines if not s.startswith(("new line", "set ", "solve"))]
    for i in range(1, 4):
        lines = [s.replace(f"sink.{i}.0", f"sink.{i}.4") for s in lines]

    def triangle(part):
        return " | ".join(
            " ".join(str(part[i, j]) for j in range(i + 1)) for i in range(4)
        )

    lines.append(
        "new line.line phases=4 bus1=source.1.2.3.4 bus2=sink.1.2.3.4 "
        f"length=1 units=none rmatrix=[{triangle(Z_LINE.real)}] "
        f"xmatrix=[{triangle(Z_LINE.imag)}] cmatrix=[{triangle(np.zeros((4, 4)))}]"
    )
    grounds = [("source", Z_SOURCE_GROUND)]
    if sink_ground:
        grounds.append(("sink", Z_SINK_GROUND))
    for bus, impedance in grounds:
        lines.append(
            f"new reactor.{bus}_ground phases=1 bus1={bus}.4 bus2={bus}.0 "
            f"R={impedance.real} X={impedance.imag}"
        )
    lines += ["set controlmode=off", "set tolerance=1e-12 maxiterations=100", "solve"]
    return "\n".join(lines) + "\n"


def analytic(source: np.ndarray, sink_ground: bool) -> dict:
    _, _, loads = cases()["coupled_unbalanced"]
    # Incidence-based nodal reference. Only the three measured source phases
    # are fixed; source neutral and all four sink conductors are solved.
    y_line = np.linalg.inv(Z_LINE)
    y = np.block([[y_line, -y_line], [-y_line, y_line]])
    for i, impedance in enumerate(loads):
        incidence = np.zeros(8)
        incidence[4 + i], incidence[7] = 1, -1
        y += np.outer(incidence, incidence) / impedance
    y[3, 3] += 1 / Z_SOURCE_GROUND
    if sink_ground:
        y[7, 7] += 1 / Z_SINK_GROUND
    v = np.concatenate([source, np.linalg.solve(y[3:, 3:], -y[3:, :3] @ source)])
    current = y_line @ (v[:4] - v[4:])
    result = {}
    for i, phase in enumerate("ABCN"):
        add(result, f"source.{phase}.voltage", "V", v[i])
        add(result, f"sink.{phase}.voltage", "V", v[4 + i])
        add(result, f"line.from.{phase}.current", "A", current[i])
        add(result, f"line.to.{phase}.current", "A", -current[i])
        add(result, f"line.from.{phase}.power", "VA", v[i] * current[i].conjugate())
        add(result, f"line.to.{phase}.power", "VA", -v[4 + i] * current[i].conjugate())
        for j, other in enumerate("ABCN"):
            add(result, f"line.z.{phase}{other}", "ohm", Z_LINE[i, j])
    add(result, "line.loss", "VA", sum((v[:4] - v[4:]) * current.conjugate()))
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": "OpenDSS explicit-neutral calibration; no GLM equivalence claim",
        "cases": {},
    }
    snapshots = {}
    for grounded in (False, True):
        name = "neutral_two_grounds" if grounded else "neutral_source_ground"
        path = output / f"{name}.dss"
        path.write_text(deck(grounded))
        actual, metadata = read_dss(path, 4)
        if LOCK["dss"]["version"] not in metadata["version"]:
            raise ValueError("DSS engine version mismatch")
        source = np.array(
            [complex(*actual[f"source.{p}.voltage"]["value"]) for p in "ABC"]
        )
        expected = analytic(source, grounded)
        comparison = compare(expected, actual)
        report["cases"][name] = dict(
            comparison,
            engine=metadata,
            input_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
        )
        (output / f"{name}.json").write_text(
            json.dumps(
                {"expected": expected, "actual": actual}, indent=2, allow_nan=False
            )
            + "\n"
        )
        snapshots[grounded] = actual
        print(name, comparison)
    # A physical negative control, solved again by the engine: removing the
    # sink earth connection must change the measured neutral/phase voltages.
    witness = "sink.N.voltage"
    defect = compare(
        {witness: snapshots[True][witness]}, {witness: snapshots[False][witness]}
    )
    report["removed_ground_detected"] = not defect["passed"]
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return (
        0
        if report["removed_ground_detected"]
        and all(r["passed"] for r in report["cases"].values())
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())

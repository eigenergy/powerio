"""OpenDSS solve oracle for distribution fixtures."""

from __future__ import annotations

import math
import os
import sys
import tempfile
from pathlib import Path

import powerio.dist as dist
from opendssdirect import dss


FIXTURES = [
    Path("tests/data/dist/micro/fourwire_linecode.dss"),
    Path("tests/data/dist/micro/linecode_10x10.dss"),
    Path("tests/data/dist/micro/neutral_grounding_reactor.dss"),
    Path("tests/data/dist/micro/onephase_cvr_load.dss"),
    Path("tests/data/dist/micro/onephase_zip_load.dss"),
    Path("tests/data/dist/micro/switch.dss"),
    Path("tests/data/dist/micro/xfmr_1ph_delta_wye.dss"),
    Path("tests/data/dist/micro/xfmr_center_tap.dss"),
    Path("tests/data/dist/micro/xfmr_delta_wye.dss"),
    Path("tests/data/dist/micro/xfmr_open_wye_open_delta.dss"),
    Path("tests/data/dist/micro/xfmr_single_phase.dss"),
    Path("tests/data/dist/micro/xfmr_wye_delta.dss"),
]

TOLERANCE_VOLTS = 1e-3


def dss_path(path: Path) -> str:
    return '"' + str(path.resolve()).replace('"', '""') + '"'


def solve_voltage_magnitudes(path: Path) -> dict[str, float]:
    dss.Basic.ClearAll()
    dss(f"Redirect {dss_path(path)}")
    dss("Solve")
    if not dss.Solution.Converged():
        raise RuntimeError("OpenDSS solve did not converge")
    names = list(dss.Circuit.AllNodeNames())
    volts = list(dss.Circuit.AllBusVolts())
    if len(volts) != 2 * len(names):
        raise RuntimeError(
            f"OpenDSS returned {len(volts)} voltage components for {len(names)} nodes"
        )
    return {
        name.lower(): math.hypot(re, im)
        for name, re, im in zip(names, volts[0::2], volts[1::2])
    }


def append_result(case: Path, mark: str) -> None:
    out = os.environ.get("PIO_RESULTS_TSV")
    if out:
        with open(out, "a", encoding="utf-8") as fh:
            fh.write(f"{case.as_posix()}\topendss\t{mark}\n")


def validate_case(case: Path) -> list[str]:
    expected = solve_voltage_magnitudes(case)
    network = dist.parse_file(case, "dss")
    generated = network.to_canonical_format("dss")
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / case.name
        path.write_text(generated.text, encoding="utf-8")
        actual = solve_voltage_magnitudes(path)

    failures: list[str] = []
    expected_nodes = set(expected)
    actual_nodes = set(actual)
    if expected_nodes != actual_nodes:
        missing = sorted(expected_nodes - actual_nodes)
        extra = sorted(actual_nodes - expected_nodes)
        failures.append(f"node set changed; missing={missing} extra={extra}")

    for node in sorted(expected_nodes & actual_nodes):
        diff = abs(expected[node] - actual[node])
        if diff > TOLERANCE_VOLTS:
            failures.append(
                f"{node}: |{expected[node]:.12g} - {actual[node]:.12g}| = {diff:.6g} V"
            )
    return failures


def main() -> int:
    failures: list[str] = []
    for case in FIXTURES:
        try:
            case_failures = validate_case(case)
        except Exception as err:  # noqa: BLE001
            case_failures = [str(err)]
        mark = "ok" if not case_failures else "FAIL"
        append_result(case, mark)
        print(f"{case}: {mark}")
        for failure in case_failures[:10]:
            print(f"  {failure}")
        if case_failures:
            failures.append(f"{case}: {len(case_failures)} mismatch(es)")

    if failures:
        print("\nOpenDSS voltage oracle failed:")
        for failure in failures:
            print(f"  {failure}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Compare PowerIO's PowerFactory DGS reader with PyPowSybl on every DGS fixture.

Every ``.dgs`` file under the pinned PowSybl Core checkout's PowerFactory
converter test resources goes through ``powerio serialize`` into PowerIO IR
and through PyPowSybl's POWER-FACTORY importer. For every fixture both tools
read, the element counts must agree: bus view buses, lines (``ElmLne`` and
``ElmZpu``), two and three winding transformers, generators (machines,
external grids, and the generation an ``ElmLodmv`` states), loads, shunts,
and HVDC lines. The IEEE 14 fixture also compares line and transformer
impedances, load and generator set points, and the capacitor bank against
PyPowSybl's values.

A fixture PyPowSybl refuses is recorded with its reason; PowerIO may read it
or refuse it, and the outcome is printed so a change in either direction is
visible in the gate log.
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
from pathlib import Path

import pypowsybl as pp

from check_outputs import check_powsybl_version, require

FIXTURE_DIRECTORY = Path("powerfactory/powerfactory-converter/src/test/resources")

# PyPowSybl reads the DC side of these fixtures through its reduced HVDC
# converter, which keeps the DC terminals of a configuration it cannot
# reduce as AC buses; PowerIO drops them with `READ.DGS.RECORD_UNMAPPED`.
# The AC element counts still agree, so only the bus count is exempt.
BUS_COUNT_EXEMPT = frozenset(
    {
        "MTDC-2-VSC",
        "MTDC-2-VSC-ACDC-links",
        "MTDC-3-VSC-ACDC-links",
        "MTDC-4-VSC-ACDC-links",
        "MTDC-ElmCoup_ACDC",
        "MTDC-ElmCoup_TypSwitch",
        "MTDC-ElmCoup_bad",
        "MTDC-ElmCoup_bad-4T",
        "MTDC-ElmCoup_no-type",
        "MTDC-ElmGndswt",
        "MTDCVscDanglingTerminal",
        "MTDCVscLoss1",
        "MTDCVscLoss2",
        "MTDCVscLoss3",
        "MTDCVscVariants1",
        "MTDCVscVariants2",
        "MTDCVscVariants3",
        "MTDCVscVariants4",
        "MTDCVscVariants5",
    }
)

IEEE14_ZBASE = 138.0**2 / 100.0
ABSOLUTE = 1e-6
RELATIVE = 1e-6


def close(a: float, b: float) -> bool:
    return math.isclose(a, b, rel_tol=RELATIVE, abs_tol=ABSOLUTE)


def serialize(powerio: Path, fixture: Path, output_dir: Path) -> tuple[dict | None, str]:
    """Run ``powerio serialize`` and return the IR document and stderr."""
    target = output_dir / f"{fixture.stem}.pio.json"
    completed = subprocess.run(
        [str(powerio), "serialize", str(fixture), "-o", str(target)],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        return None, completed.stderr.strip()
    with target.open(encoding="utf-8") as handle:
        return json.load(handle), completed.stderr.strip()


def powerio_counts(document: dict) -> dict[str, int]:
    value = document["value"]
    data = value["data"]
    if value["type"] == "powerio.MulticonductorNetwork":
        return {
            "family": "multiconductor",
            "buses": len(data["buses"]),
            "lines": len(data["lines"]),
            "transformers_2w": sum(1 for t in data["transformers"] if len(t["windings"]) == 2),
            "transformers_3w": sum(1 for t in data["transformers"] if len(t["windings"]) == 3),
            "generators": len(data["generators"]) + len(data["sources"]),
            "loads": len(data["loads"]),
            "shunts": len(data["shunts"]),
            "hvdc": 0,
        }
    branches = data["branches"]
    return {
        "family": "balanced",
        "buses": len(data["buses"]),
        "lines": sum(1 for b in branches if b["extras"].get("dgs.class") in ("ElmLne", "ElmZpu")),
        "transformers_2w": sum(1 for b in branches if b["extras"].get("dgs.class") == "ElmTr2"),
        "transformers_3w": len(data["transformers_3w"]),
        "generators": len(data["generators"]),
        "loads": len(data["loads"]),
        "shunts": len(data["shunts"]),
        "hvdc": len(data["hvdc"]),
    }


def powsybl_counts(network: pp.network.Network) -> dict[str, int]:
    return {
        "buses": len(network.get_buses()),
        "lines": len(network.get_lines()),
        "transformers_2w": len(network.get_2_windings_transformers()),
        "transformers_3w": len(network.get_3_windings_transformers()),
        "generators": len(network.get_generators()),
        "loads": len(network.get_loads()),
        "shunts": len(network.get_shunt_compensators()),
        "hvdc": len(network.get_hvdc_lines()),
    }


def check_ieee14_values(document: dict, network: pp.network.Network) -> int:
    """Compare the IEEE 14 electrical values; returns the number of values."""
    data = document["value"]["data"]
    compared = 0
    lines = network.get_lines()
    transformers = network.get_2_windings_transformers()
    for branch in data["branches"]:
        name = branch["name"]
        if branch["extras"]["dgs.class"] == "ElmLne":
            row = lines.loc[name]
            require(
                close(branch["r"] * IEEE14_ZBASE, row["r"]),
                f"ieee14 line {name}: r {branch['r'] * IEEE14_ZBASE} vs PowSybl {row['r']}",
            )
            require(
                close(branch["x"] * IEEE14_ZBASE, row["x"]),
                f"ieee14 line {name}: x {branch['x'] * IEEE14_ZBASE} vs PowSybl {row['x']}",
            )
            charging = branch["charging"]
            require(
                close(charging["b_fr"] / IEEE14_ZBASE, row["b1"])
                and close(charging["b_to"] / IEEE14_ZBASE, row["b2"]),
                f"ieee14 line {name}: charging {charging} vs PowSybl {row['b1']}, {row['b2']}",
            )
            compared += 4
        else:
            row = transformers.loc[name]
            # PowSybl refers the leakage impedance to its side 2 nominal
            # voltage; both sides are 138 kV here.
            require(
                close(branch["r"] * IEEE14_ZBASE, row["r"])
                and close(branch["x"] * IEEE14_ZBASE, row["x"]),
                f"ieee14 transformer {name}: z {branch['r']}, {branch['x']} vs PowSybl "
                f"{row['r']}, {row['x']}",
            )
            # The structural ratio: rated_u1 / rated_u2 against equal
            # nominal voltages equals the MATPOWER tap.
            require(
                close(branch["tap"], row["rated_u1"] / row["rated_u2"]),
                f"ieee14 transformer {name}: tap {branch['tap']} vs PowSybl "
                f"{row['rated_u1'] / row['rated_u2']}",
            )
            compared += 3
    loads = network.get_loads()
    for load in data["loads"]:
        row = loads.loc[load["uid"]]
        require(
            close(load["p"], row["p0"]) and close(load["q"], row["q0"]),
            f"ieee14 load {load['uid']}: {load['p']}, {load['q']} vs PowSybl {row['p0']}, {row['q0']}",
        )
        compared += 2
    generators = network.get_generators()
    buses = {bus["id"]: bus for bus in data["buses"]}
    for generator in data["generators"]:
        row = generators.loc[generator["uid"]]
        base_kv = buses[generator["bus"]]["base_kv"]
        require(
            close(generator["pg"], row["target_p"])
            and close(generator["qg"], row["target_q"])
            and math.isclose(generator["vg"] * base_kv, row["target_v"], rel_tol=1e-6),
            f"ieee14 generator {generator['uid']}: set points differ from PowSybl",
        )
        require(
            math.isclose(generator["qmin"], row["min_q"], rel_tol=1e-6, abs_tol=1e-4)
            and math.isclose(generator["qmax"], row["max_q"], rel_tol=1e-6, abs_tol=1e-4),
            f"ieee14 generator {generator['uid']}: reactive limits {generator['qmin']}, "
            f"{generator['qmax']} vs PowSybl {row['min_q']}, {row['max_q']}",
        )
        require(
            bool(generator["voltage_regulation_on"]) == bool(row["voltage_regulator_on"]),
            f"ieee14 generator {generator['uid']}: voltage regulation flag differs",
        )
        compared += 6
    shunts = network.get_shunt_compensators()
    for shunt in data["shunts"]:
        row = shunts.loc[shunt["uid"]]
        base_kv = buses[shunt["bus"]]["base_kv"]
        require(
            close(shunt["b"] / base_kv**2, row["b"]) and close(shunt["g"] / base_kv**2, row["g"]),
            f"ieee14 shunt {shunt['uid']}: {shunt['g']}, {shunt['b']} vs PowSybl {row['g']}, {row['b']}",
        )
        compared += 2
    return compared


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("powerio", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("powsybl_core", type=Path)
    args = parser.parse_args()
    check_powsybl_version()

    fixtures = sorted((args.powsybl_core.resolve() / FIXTURE_DIRECTORY).glob("*.dgs"))
    require(len(fixtures) >= 90, f"expected at least 90 DGS fixtures, found {len(fixtures)}")
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)

    compared = 0
    refused_by_powsybl = 0
    refused_by_powerio = 0
    ieee14_values = 0
    for fixture in fixtures:
        document, stderr = serialize(args.powerio, fixture, output_dir)
        try:
            network = pp.network.load(fixture)
            powsybl = powsybl_counts(network)
        except Exception as error:  # noqa: BLE001 - PyPowSybl raises its own hierarchy
            network = None
            powsybl = None
            reason = str(error).splitlines()[0][:160]
            refused_by_powsybl += 1
        if document is None:
            refused_by_powerio += 1
            reason_powerio = stderr.splitlines()[-1][:160] if stderr else "no output"
            print(f"{fixture.stem}: PowerIO refused: {reason_powerio}")
            require(
                network is None,
                f"{fixture.stem}: PyPowSybl reads the fixture but PowerIO refused it: {stderr}",
            )
            print(f"{fixture.stem}: PyPowSybl refused: {reason}")
            continue
        mine = powerio_counts(document)
        family = mine.pop("family")
        if network is None:
            print(f"{fixture.stem}: PowerIO read a {family} network; PyPowSybl refused: {reason}")
            continue
        differences = []
        for key, value in powsybl.items():
            if key == "buses" and fixture.stem in BUS_COUNT_EXEMPT:
                continue
            if mine[key] != value:
                differences.append(f"{key} {mine[key]} vs PowSybl {value}")
        require(
            not differences,
            f"{fixture.stem}: element counts differ: {'; '.join(differences)}",
        )
        compared += 1
        print(f"{fixture.stem}: {family}, counts agree {mine}")
        if fixture.stem == "ieee14":
            ieee14_values = check_ieee14_values(document, network)
    require(ieee14_values > 0, "ieee14.dgs values were not compared")
    print(
        f"DGS gate: {compared} fixtures with agreeing counts, {ieee14_values} IEEE 14 values, "
        f"{refused_by_powsybl} refused by PyPowSybl, {refused_by_powerio} refused by PowerIO"
    )


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        print(f"DGS gate failed: {error}", file=sys.stderr)
        sys.exit(1)

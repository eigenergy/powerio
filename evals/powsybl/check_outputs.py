#!/usr/bin/env python3
"""Load fresh PowerIO output with PowSyBl and check the imported network."""

from __future__ import annotations

import argparse
import contextlib
import io
import shutil
from pathlib import Path

import pypowsybl as pp


EXPECTED_BUSES = 9
EXPECTED_BRANCHES = 9
EXPECTED_GENERATORS = 3
EXPECTED_LOADS = 3
EXPECTED_PYPOWSYBL_VERSION = "1.16.1"
EXPECTED_POWSYBL_CORE_VERSION = "7.3.0"
EXPECTED_POWSYBL_CORE_COMMIT = "0939bfcc2c0c094de907dc818dd688b4cbfb7281"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def check_powsybl_version() -> None:
    require(
        pp.__version__ == EXPECTED_PYPOWSYBL_VERSION,
        f"PyPowSyBl {pp.__version__}, expected {EXPECTED_PYPOWSYBL_VERSION}",
    )
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        pp.print_version()
    version_rows = {
        fields[0]: (fields[1], fields[3])
        for line in output.getvalue().splitlines()
        if len(fields := [field.strip() for field in line.split("|") if field.strip()]) >= 4
    }
    require(
        version_rows.get("powsybl-core")
        == (EXPECTED_POWSYBL_CORE_VERSION, EXPECTED_POWSYBL_CORE_COMMIT),
        "PyPowSyBl does not contain the pinned PowSyBl Core 7.3.0 build",
    )


def count_branches(network: pp.network.Network) -> int:
    return sum(
        len(frame)
        for frame in (
            network.get_lines(),
            network.get_2_windings_transformers(),
            network.get_3_windings_transformers(),
            network.get_tie_lines(),
            network.get_boundary_lines(),
        )
    )


def check_references(network: pp.network.Network, label: str) -> None:
    identifiables = network.get_identifiables()
    require(not identifiables.index.has_duplicates, f"{label}: duplicate identifiable ids")
    identifiable_ids = set(identifiables.index)

    voltage_levels = network.get_voltage_levels()
    voltage_level_ids = set(voltage_levels.index)
    require(voltage_level_ids, f"{label}: no voltage levels")

    bus_breaker_buses = network.get_bus_breaker_view_buses()
    require(len(bus_breaker_buses) > 0, f"{label}: no bus breaker buses")
    require(
        set(bus_breaker_buses["voltage_level_id"]).issubset(voltage_level_ids),
        f"{label}: a bus references an unknown voltage level",
    )

    buses = network.get_buses()
    bus_ids = set(buses.index)
    require(bus_ids, f"{label}: no calculated buses")

    terminals = network.get_terminals()
    require(len(terminals) > 0, f"{label}: no terminals")
    require(
        set(terminals.index).issubset(identifiable_ids),
        f"{label}: a terminal references an unknown element",
    )
    require(
        set(terminals["voltage_level_id"]).issubset(voltage_level_ids),
        f"{label}: a terminal references an unknown voltage level",
    )

    connected = terminals[terminals["connected"]]
    require(
        not connected["bus_id"].isna().any(),
        f"{label}: a connected terminal has no bus",
    )
    require(
        set(connected["bus_id"]).issubset(bus_ids),
        f"{label}: a connected terminal references an unknown bus",
    )


def check_network(path: Path, label: str) -> None:
    network = pp.network.load(path)
    validation_level = network.validate()

    bus_count = len(network.get_buses())
    bus_breaker_count = len(network.get_bus_breaker_view_buses())
    branch_count = count_branches(network)
    generator_count = len(network.get_generators())
    load_count = len(network.get_loads())

    require(bus_count == EXPECTED_BUSES, f"{label}: {bus_count} buses, expected 9")
    require(
        bus_breaker_count == EXPECTED_BUSES,
        f"{label}: {bus_breaker_count} bus breaker buses, expected 9",
    )
    require(
        branch_count == EXPECTED_BRANCHES,
        f"{label}: {branch_count} branches, expected 9",
    )
    require(
        generator_count == EXPECTED_GENERATORS,
        f"{label}: {generator_count} generators, expected 3",
    )
    require(load_count == EXPECTED_LOADS, f"{label}: {load_count} loads, expected 3")
    check_references(network, label)

    print(
        f"{label}: source={network.source_format}, validation={validation_level.name}, "
        f"buses={bus_count}, branches={branch_count}, "
        f"generators={generator_count}, loads={load_count}"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output_dir", type=Path)
    args = parser.parse_args()
    check_powsybl_version()

    output_dir = args.output_dir.resolve()
    cgmes_directory = output_dir / "case9-cgmes"
    cgmes_zip = Path(
        shutil.make_archive(
            str(output_dir / "case9-cgmes"),
            "zip",
            root_dir=cgmes_directory,
        )
    )

    for path, label in (
        (output_dir / "case9.xiidm", "XIIDM"),
        (cgmes_zip, "CGMES"),
        (output_dir / "case9.raw", "PSS/E RAW"),
        (output_dir / "case9.rawx", "PSS/E RAWX"),
    ):
        require(path.exists(), f"{label}: missing output {path}")
        check_network(path, label)


if __name__ == "__main__":
    main()

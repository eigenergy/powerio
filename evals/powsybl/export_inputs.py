#!/usr/bin/env python3
"""Export PowSybl generated XIIDM and CGMES inputs for the external gate."""

from __future__ import annotations

import argparse
from pathlib import Path

from check_outputs import (
    REMOTE_CONTROL_RELATIVE,
    check_powsybl_version,
    load_checked,
)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("powsybl_core", type=Path)
    parser.add_argument("output_dir", type=Path)
    args = parser.parse_args()

    check_powsybl_version()
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    network = load_checked(
        args.powsybl_core.resolve() / REMOTE_CONTROL_RELATIVE,
        "PowSybl export source",
    )

    for version in range(12, 18):
        network.save(
            output_dir / f"powsybl-xiidm-1-{version}.xiidm",
            "XIIDM",
            {"iidm.export.xml.version": f"1.{version}"},
        )

    for cim_version, label in (("16", "2415"), ("100", "30")):
        network.save(
            output_dir / f"powsybl-cgmes-{label}.zip",
            "CGMES",
            {
                "iidm.export.cgmes.base-name": f"powsybl-cgmes-{label}",
                "iidm.export.cgmes.cim-version": cim_version,
                "iidm.export.cgmes.profiles": "EQ,TP,SSH,SV",
            },
        )


if __name__ == "__main__":
    main()

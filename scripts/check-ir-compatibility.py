"""Check current IR 2 output with the published PowerIO 0.11.0 reader."""

import argparse
import io
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import powerio

ROOT = Path(__file__).resolve().parents[1]
# Structural types added after 0.11.0 under the same IR version. A 0.11.0
# reader rejects each by name and keeps reading the types it implements.
ADDITIVE_TYPES = {
    "powerio.ContingencySet",
    "powerio.LinDist3FlowOpfInstance",
    "powerio.LinDist3FlowOpfSolution",
    "powerio.MonitoredSet",
    "powerio.SubsystemSet",
}
# Records that gained an optional field after 0.11.0. The power flow
# instances `to_dc_pf_instance` and `to_ac_pf_instance` build carry HVDC lines
# as fixed injections, which their documents state as `hvdc_treatment`; a
# 0.11.0 reader rejects the field as unknown rather than read the document
# under a different meaning. An `ignore` instance is written without the field.
# Without it the record is the one 0.11.0 wrote, and 0.11.0 reads it.
ADDED_FIELDS = {
    "powerio.AcPfInstance": "hvdc_treatment",
    "powerio.DcPfInstance": "hvdc_treatment",
}


def read_with_legacy(directory: Path) -> None:
    assert powerio.__version__ == "0.11.0", powerio.__version__
    accepted = 0
    rejected = set()
    rejected_fields = set()
    for path in sorted(directory.glob("*.json")):
        document = json.loads(path.read_text())
        kind = document["value"]["type"]
        assert document["version"] == 2, path
        if kind in ADDED_FIELDS:
            field = ADDED_FIELDS[kind]
            assert field in document["value"]["data"], (path, field)
            try:
                powerio.deserialize(path)
            except powerio.PowerIOError as error:
                assert "unknown field" in str(error) and field in str(error), error
                rejected_fields.add(kind)
            else:
                raise AssertionError(f"0.11.0 unexpectedly read {kind} with {field}")
            del document["value"]["data"][field]
            without = directory / "without-field" / path.name
            without.parent.mkdir(exist_ok=True)
            without.write_text(json.dumps(document))
            module = powerio.deserialize(without)
            decoded = json.loads(powerio.serialize(module).text)
            assert decoded["value"] == document["value"], path
            accepted += 1
        elif kind in ADDITIVE_TYPES:
            try:
                powerio.deserialize(path)
            except powerio.PowerIOError as error:
                assert "unknown variant" in str(error) and kind in str(error), error
                rejected.add(kind)
            else:
                raise AssertionError(f"0.11.0 unexpectedly implemented {kind}")
        else:
            module = powerio.deserialize(path)
            decoded = json.loads(powerio.serialize(module).text)
            assert decoded["value"] == document["value"], path
            accepted += 1
    assert accepted == 8 and rejected == ADDITIVE_TYPES, (accepted, sorted(rejected))
    assert rejected_fields == set(ADDED_FIELDS), sorted(rejected_fields)
    print(
        f"IR 2 compatibility: 0.11.0 read {accepted} existing types "
        f"and rejected only the {len(rejected)} new types and the "
        f"{len(rejected_fields)} records stating a new field"
    )


def check_output(legacy_package: Path) -> None:
    assert powerio.versions()["powerio_ir"] == {"schema": "pio-ir", "version": 2}
    balanced = powerio.parse(ROOT / "tests/data/case9.m")
    solution = powerio.deserialize(ROOT / "tests/data/dist/micro/lindist3flow-solution.pio.json")
    document = json.loads(powerio.serialize(solution).text)
    network = document["value"]["data"]["instance"]["base"]["network"]
    document["value"] = {"type": "powerio.MulticonductorNetwork", "data": network}
    multiconductor = powerio.deserialize(io.StringIO(json.dumps(document)))
    contingency = ROOT / "tests/data/psse/contingency"
    modules = [
        balanced,
        balanced.to_dc_pf_instance(),
        balanced.to_ac_pf_instance(),
        balanced.to_dc_opf_instance(),
        balanced.to_ac_opf_instance(),
        multiconductor,
        multiconductor.to_mc_ac_pf_instance(),
        multiconductor.to_mc_ac_opf_instance(),
        multiconductor.to_lindist3flow_opf_instance(),
        solution,
        powerio.parse(contingency / "psse35_generated.con"),
        powerio.parse(contingency / "psse35_area.sub"),
        powerio.parse(contingency / "generated.mon"),
    ]
    with tempfile.TemporaryDirectory(prefix="powerio-ir2-compat-") as temporary:
        directory = Path(temporary)
        for index, module in enumerate(modules):
            text = powerio.serialize(module).text
            assert json.loads(text)["version"] == 2
            (directory / f"{index}.json").write_text(text)
        environment = dict(os.environ, PYTHONPATH=str(legacy_package.resolve()))
        subprocess.run(
            [sys.executable, str(Path(__file__).resolve()), "--read", str(directory)],
            env=environment,
            cwd=directory,
            check=True,
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--legacy-package", type=Path)
    modes.add_argument("--read", type=Path)
    arguments = parser.parse_args()
    if arguments.read is not None:
        read_with_legacy(arguments.read)
    else:
        check_output(arguments.legacy_package)


if __name__ == "__main__":
    main()

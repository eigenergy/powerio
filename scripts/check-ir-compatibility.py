"""Check that this build reads the PowerIO IR documents published 0.11
releases wrote, and the documents of its own release series.

Each --legacy-package is a directory holding one published 0.11 wheel's
`powerio` package. The script runs it in a subprocess (--write) to write one
document per structural type that release writes, then reads every document
with the installed build: it accepts the integer version 2, and the value
it writes back is the one the 0.11 release wrote.
"""

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
FIXTURE = ROOT / "tests/data/dist/micro/lindist3flow-solution.pio.json"
CONTINGENCY = ROOT / "tests/data/psse/contingency"


def release(text: str) -> tuple:
    return tuple(int(part) for part in text.split("."))


def write_with_legacy(directory: Path) -> None:
    """Write one document per structural type, under a 0.11 package."""
    legacy = release(powerio.__version__)
    assert legacy[:2] == (0, 11), powerio.__version__
    balanced = powerio.parse(ROOT / "tests/data/case9.m")
    document = json.loads(FIXTURE.read_text())
    network = document["value"]["data"]["instance"]["base"]["network"]
    document["value"] = {"type": "powerio.MulticonductorNetwork", "data": network}
    multiconductor = powerio.deserialize(io.StringIO(json.dumps(document)))
    # Each module with the first 0.11 release that writes its type.
    builders = [
        ("0.11.0", lambda: balanced),
        ("0.11.0", lambda: balanced.to_dc_pf_instance()),
        ("0.11.0", lambda: balanced.to_ac_pf_instance()),
        ("0.11.0", lambda: balanced.to_dc_opf_instance()),
        ("0.11.0", lambda: balanced.to_ac_opf_instance()),
        ("0.11.0", lambda: multiconductor),
        ("0.11.0", lambda: multiconductor.to_mc_ac_pf_instance()),
        ("0.11.0", lambda: multiconductor.to_mc_ac_opf_instance()),
        ("0.11.1", lambda: multiconductor.to_lindist3flow_opf_instance()),
        ("0.11.1", lambda: powerio.deserialize(FIXTURE)),
        ("0.11.3", lambda: powerio.parse(CONTINGENCY / "psse35_generated.con")),
        ("0.11.3", lambda: powerio.parse(CONTINGENCY / "psse35_area.sub")),
        ("0.11.3", lambda: powerio.parse(CONTINGENCY / "generated.mon")),
        ("0.11.4", lambda: multiconductor.to_lindist3flow_pf_instance()),
    ]
    for index, (first, build) in enumerate(builders):
        if legacy >= release(first):
            text = powerio.serialize(build()).text
            (directory / f"{index:02}.json").write_text(text)


def read_legacy_documents(legacy_package: Path) -> None:
    """Read with this build what one 0.11 package writes."""
    with tempfile.TemporaryDirectory(prefix="powerio-ir-0.11-") as temporary:
        directory = Path(temporary)
        environment = dict(os.environ, PYTHONPATH=str(legacy_package.resolve()))
        subprocess.run(
            [sys.executable, str(Path(__file__).resolve()), "--write", str(directory)],
            env=environment,
            cwd=directory,
            check=True,
        )
        types = set()
        for path in sorted(directory.glob("*.json")):
            document = json.loads(path.read_text())
            assert document["version"] == 2, path
            module = powerio.deserialize(path)
            rewritten = json.loads(powerio.serialize(module).text)
            assert rewritten["version"] == powerio.versions()["powerio_ir"]["version"]
            assert rewritten["value"] == document["value"], document["value"]["type"]
            types.add(document["value"]["type"])
        assert len(types) >= 8, sorted(types)
        # The parsed case comes first; its producer is the 0.11 release.
        producer = json.loads((directory / "00.json").read_text())["producer"]
    print(
        f"PowerIO IR compatibility: read {len(types)} structural types "
        f"from {legacy_package.name} ({producer['name']} {producer['version']})"
    )


def check_release_series() -> None:
    """This build writes its release and reads its own release series."""
    stated = powerio.versions()["powerio_ir"]
    current = stated["version"]
    assert stated["schema"] == "pio-ir" and isinstance(current, str), stated
    major, minor, patch = release(current)
    document = json.loads(powerio.serialize(powerio.parse(ROOT / "tests/data/case9.m")).text)
    assert document["version"] == current, document["version"]
    if major == 0:
        later, next_series = f"0.{minor}.{patch + 7}", f"0.{minor + 1}.0"
    else:
        later, next_series = f"{major}.{minor + 1}.0", f"{major + 1}.0.0"
    document["version"] = later
    module = powerio.deserialize(io.StringIO(json.dumps(document)))
    assert json.loads(powerio.serialize(module).text)["value"] == document["value"]
    document["version"] = next_series
    try:
        powerio.deserialize(io.StringIO(json.dumps(document)))
    except powerio.PowerIOError as error:
        assert error.code == "READ.MODULE.UNSUPPORTED", error
        assert "upgrade PowerIO" in str(error), error
    else:
        raise AssertionError(f"{current} read a {next_series} document")
    print(f"PowerIO IR {current}: reads {later} and refuses {next_series}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--legacy-package", type=Path, action="append")
    modes.add_argument("--write", type=Path, help=argparse.SUPPRESS)
    arguments = parser.parse_args()
    if arguments.write is not None:
        write_with_legacy(arguments.write)
        return
    check_release_series()
    for legacy_package in arguments.legacy_package:
        read_legacy_documents(legacy_package)


if __name__ == "__main__":
    main()

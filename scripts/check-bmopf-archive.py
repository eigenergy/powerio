#!/usr/bin/env python3
"""Check the archived schema identities and byte hashes without network access."""

import argparse
import hashlib
import json
import shutil
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ARCHIVE = ROOT / "powerio-dist/schemas/bmopf"


def check_package(path):
    if path.suffix == ".whl":
        with zipfile.ZipFile(path) as package:
            files = {name: package.read(name) for name in package.namelist() if "/schemas/bmopf/" in name}
    else:
        with tarfile.open(path, "r:*") as package:
            files = {item.name: package.extractfile(item).read() for item in package.getmembers()
                     if item.isfile() and "schemas/bmopf/" in item.name}
    for source in ARCHIVE.rglob("*"):
        if source.is_file():
            suffix = "schemas/bmopf/" + source.relative_to(ARCHIVE).as_posix()
            copies = [data for name, data in files.items() if name.endswith(suffix)]
            assert copies and all(data == source.read_bytes() for data in copies), (path, suffix)
    print("Packaged BMOPF archive verified:", path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sync-python", action="store_true")
    parser.add_argument("--archive", action="append", default=[])
    args = parser.parse_args()
    packaged = ROOT / "python/powerio/schemas/bmopf"
    if args.sync_python:
        shutil.copytree(ARCHIVE, packaged, dirs_exist_ok=True)
    for source in ARCHIVE.rglob("*"):
        if source.is_file():
            assert (packaged / source.relative_to(ARCHIVE)).read_bytes() == source.read_bytes(), source
    for name in args.archive:
        check_package(Path(name))
    manifest = json.loads((ARCHIVE / "manifest.json").read_text())
    assert manifest["format"] == 1
    assert {entry["version"] for entry in manifest["schemas"]} == {"0.1.0", "0.2.0"}
    for entry in manifest["schemas"]:
        data = (ARCHIVE / entry["path"]).read_bytes()
        assert hashlib.sha256(data).hexdigest() == entry["sha256"], entry["path"]
        assert json.loads(data)["$id"] == entry["schema_id"], entry["path"]
        assert entry["license"] == "CC-BY-4.0"
    historical = json.loads((ROOT / "tests/data/dist/bmopf/draft_bmopf_schema.json").read_text())
    baseline = json.loads((ARCHIVE / "0.1.0/bmopf.schema.json").read_text())
    historical.pop("$id")
    baseline.pop("$id")
    assert historical == baseline, "baseline validation rules changed"
    assert "CC-BY-4.0" in (ARCHIVE / "LICENSE").read_text()
    print("BMOPF schema archive: identities, hashes, license, and baseline rules verified")


if __name__ == "__main__":
    main()

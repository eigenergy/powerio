#!/usr/bin/env python3
"""Check the archived schema identities and byte hashes without network access."""

import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ARCHIVE = ROOT / "powerio-dist/schemas/bmopf"


def main():
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
